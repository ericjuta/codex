use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;

use codex_utils_path_uri::PathUri;
use futures::executor::block_on;
use pretty_assertions::assert_eq;

use super::executor_test_support::TestEvent;
use super::executor_test_support::TestFileSystem;
use super::*;

#[test]
fn every_durable_create_state_converges_after_restart() {
    let cases = [
        (1, JournalState::Preparing, RecoveryOutcome::RolledBack),
        (2, JournalState::Prepared, RecoveryOutcome::RolledBack),
        (3, JournalState::Committing, RecoveryOutcome::RolledBack),
        (4, JournalState::Committing, RecoveryOutcome::RolledBack),
        (5, JournalState::Committing, RecoveryOutcome::RolledBack),
        (6, JournalState::Committed, RecoveryOutcome::Committed),
        (7, JournalState::Cleaning, RecoveryOutcome::Committed),
        (8, JournalState::Complete, RecoveryOutcome::Committed),
    ];

    for (persist_call, durable_state, expected_outcome) in cases {
        let transaction_id = format!("tx-persist-{persist_call}");
        let file_system = TestFileSystem::new([]);
        let plan = block_on(plan(&file_system, create_request(&["created"]))).unwrap();
        let plan_digest = plan.plan_digest;
        file_system.crash_after_persist_at(persist_call);

        assert_crashes(|| {
            let _ = block_on(execute(
                &file_system,
                plan,
                TransactionId(transaction_id.clone()),
                TransactionLimits::default(),
            ));
        });
        assert_eq!(file_system.journals().last().unwrap().state, durable_state);

        let result = block_on(recover_transaction(
            &file_system.restart(),
            &TestFileSystem::transaction_key(&transaction_id),
            TransactionLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            result,
            RecoveryResult {
                transaction_id: TransactionId(transaction_id),
                plan_digest,
                outcome: expected_outcome,
            },
            "persist call {persist_call}"
        );
        let expected_files = match expected_outcome {
            RecoveryOutcome::Committed => {
                BTreeMap::from([("created".to_string(), b"contents-created".to_vec())])
            }
            RecoveryOutcome::RolledBack => BTreeMap::new(),
        };
        assert_eq!(
            file_system.files(),
            expected_files,
            "persist call {persist_call}"
        );
        assert_eq!(file_system.artifact_counts(), (0, 0));
        assert_eq!(
            file_system.journals().last().unwrap().state,
            JournalState::Complete
        );
    }
}

#[test]
fn recovery_restores_apply_that_was_visible_before_applied_was_persisted() {
    let (file_system, transaction_id, plan_digest) = crashed_create_after_apply("tx-after-apply");
    assert_eq!(
        file_system.files(),
        BTreeMap::from([("created".to_string(), b"contents-created".to_vec())])
    );
    assert_eq!(
        file_system.journals().last().unwrap().mutations[0].progress,
        MutationProgress::Committing
    );

    let result = recover(&file_system, &transaction_id);

    assert_eq!(
        result,
        RecoveryResult {
            transaction_id: TransactionId(transaction_id),
            plan_digest,
            outcome: RecoveryOutcome::RolledBack,
        }
    );
    assert_eq!(file_system.files(), BTreeMap::new());
    assert_eq!(file_system.artifact_counts(), (0, 0));
}

#[test]
fn commit_cleanup_resumes_after_artifacts_were_durably_removed() {
    let file_system = TestFileSystem::new([]);
    let plan = block_on(plan(&file_system, create_request(&["created"]))).unwrap();
    let plan_digest = plan.plan_digest;
    let transaction_id = "tx-cleanup";
    file_system.crash_after_cleanup();

    assert_crashes(|| {
        let _ = block_on(execute(
            &file_system,
            plan,
            TransactionId(transaction_id.to_string()),
            TransactionLimits::default(),
        ));
    });
    assert_eq!(
        file_system.journals().last().unwrap().state,
        JournalState::Cleaning
    );
    assert_eq!(file_system.artifact_counts(), (0, 0));

    assert_eq!(
        recover(&file_system, transaction_id),
        RecoveryResult {
            transaction_id: TransactionId(transaction_id.to_string()),
            plan_digest,
            outcome: RecoveryOutcome::Committed,
        }
    );
    assert_eq!(
        file_system.journals().last().unwrap().state,
        JournalState::Complete
    );
    assert_eq!(
        file_system
            .events()
            .iter()
            .filter(|event| **event == TestEvent::Cleaned)
            .count(),
        2
    );
}

#[test]
fn rollback_recovery_retries_visible_restore_and_continues_in_reverse_order() {
    let file_system = TestFileSystem::new([]);
    let plan = block_on(plan(
        &file_system,
        create_request(&["a-created", "b-created", "c-created"]),
    ))
    .unwrap();
    let plan_digest = plan.plan_digest;
    let transaction_id = "tx-reverse-rollback";
    file_system.fail_apply_at(/*call*/ 3);
    file_system.crash_after_restore_at(/*call*/ 2);

    assert_crashes(|| {
        let _ = block_on(execute(
            &file_system,
            plan,
            TransactionId(transaction_id.to_string()),
            TransactionLimits::default(),
        ));
    });
    assert_eq!(
        file_system.files(),
        BTreeMap::from([("a-created".to_string(), b"contents-a-created".to_vec())])
    );

    assert_eq!(
        recover(&file_system, transaction_id),
        RecoveryResult {
            transaction_id: TransactionId(transaction_id.to_string()),
            plan_digest,
            outcome: RecoveryOutcome::RolledBack,
        }
    );
    assert_eq!(file_system.files(), BTreeMap::new());
    assert_eq!(
        file_system
            .events()
            .into_iter()
            .filter_map(|event| match event {
                TestEvent::Restored(path) => Some(path),
                TestEvent::Locked(_) | TestEvent::Applied(_) | TestEvent::Cleaned => None,
            })
            .collect::<Vec<_>>(),
        vec![
            "create:c-created".to_string(),
            "create:b-created".to_string(),
            "create:b-created".to_string(),
            "create:a-created".to_string(),
        ]
    );
}

#[test]
fn completed_recovery_is_idempotent() {
    let (file_system, transaction_id, plan_digest) = crashed_create_after_apply("tx-idempotent");

    let first = recover(&file_system, &transaction_id);
    let journal_count = file_system.journals().len();
    let second = recover(&file_system, &transaction_id);

    let expected = RecoveryResult {
        transaction_id: TransactionId(transaction_id),
        plan_digest,
        outcome: RecoveryOutcome::RolledBack,
    };
    assert_eq!(first, expected);
    assert_eq!(second, expected);
    assert_eq!(file_system.files(), BTreeMap::new());
    assert_eq!(file_system.artifact_counts(), (0, 0));
    assert_eq!(file_system.journals().len(), journal_count);
}

#[test]
fn pending_scan_recovers_each_reported_transaction() {
    let file_system = TestFileSystem::new([]);
    let plan = block_on(plan(&file_system, create_request(&["created"]))).unwrap();
    let plan_digest = plan.plan_digest;
    let transaction_id = "tx-pending";
    let key = TestFileSystem::transaction_key(transaction_id);
    file_system.crash_after_persist_at(/*call*/ 4);
    assert_crashes(|| {
        let _ = block_on(execute(
            &file_system,
            plan,
            TransactionId(transaction_id.to_string()),
            TransactionLimits::default(),
        ));
    });

    assert_eq!(
        block_on(recover_pending(
            &file_system.restart(),
            RecoveryScanLimit {
                max_transactions: 8,
            },
            TransactionLimits::default(),
        ))
        .unwrap(),
        vec![RecoveryAttempt {
            key,
            result: Ok(RecoveryResult {
                transaction_id: TransactionId(transaction_id.to_string()),
                plan_digest,
                outcome: RecoveryOutcome::RolledBack,
            }),
        }]
    );
    assert_eq!(file_system.files(), BTreeMap::new());
    assert_eq!(file_system.artifact_counts(), (0, 0));
}

#[test]
fn external_disturbance_requires_recovery_and_retains_evidence() {
    for (suffix, contents) in [
        ("bytes", b"external".as_slice()),
        ("identity", b"contents-created".as_slice()),
    ] {
        let transaction_id = format!("tx-disturbed-{suffix}");
        let (file_system, transaction_id, _) = crashed_create_after_apply(&transaction_id);
        file_system.external_write("created", contents, /*identity*/ 99);

        assert_eq!(
            block_on(recover_transaction(
                &file_system.restart(),
                &TestFileSystem::transaction_key(&transaction_id),
                TransactionLimits::default(),
            ))
            .unwrap_err(),
            RecoveryError::RecoveryRequired {
                transaction_id: TransactionId(transaction_id),
                failure: RecoveryFailure::FileSystem(
                    TransactionFileSystemError::ChangedSincePlanning {
                        path: "rollback create:created".to_string(),
                    },
                ),
                record_failure: None,
            },
            "{suffix}"
        );
        assert_eq!(
            file_system.files(),
            BTreeMap::from([("created".to_string(), contents.to_vec())]),
            "{suffix}"
        );
        assert_eq!(file_system.artifact_counts(), (1, 0), "{suffix}");
        assert_eq!(
            file_system.journals().last().unwrap().state,
            JournalState::RecoveryRequired,
            "{suffix}"
        );
    }
}

#[test]
fn mixed_rollback_recovery_restores_every_operation_kind() {
    let file_system = mixed_file_system();
    let plan = block_on(plan(&file_system, mixed_request())).unwrap();
    let plan_digest = plan.plan_digest;
    let transaction_id = "tx-mixed-rollback";
    file_system.crash_after_apply_at(/*call*/ 4);

    assert_crashes(|| {
        let _ = block_on(execute(
            &file_system,
            plan,
            TransactionId(transaction_id.to_string()),
            TransactionLimits::default(),
        ));
    });
    assert_eq!(file_system.files(), committed_mixed_files());

    assert_eq!(
        recover(&file_system, transaction_id),
        RecoveryResult {
            transaction_id: TransactionId(transaction_id.to_string()),
            plan_digest,
            outcome: RecoveryOutcome::RolledBack,
        }
    );
    assert_eq!(file_system.files(), original_mixed_files());
    assert_eq!(file_system.artifact_counts(), (0, 0));
}

#[test]
fn mixed_commit_recovery_verifies_every_operation_kind() {
    let file_system = mixed_file_system();
    let plan = block_on(plan(&file_system, mixed_request())).unwrap();
    let plan_digest = plan.plan_digest;
    let transaction_id = "tx-mixed-commit";
    file_system.crash_after_persist_at(/*call*/ 12);

    assert_crashes(|| {
        let _ = block_on(execute(
            &file_system,
            plan,
            TransactionId(transaction_id.to_string()),
            TransactionLimits::default(),
        ));
    });
    assert_eq!(
        file_system.journals().last().unwrap().state,
        JournalState::Committed
    );
    assert_eq!(file_system.files(), committed_mixed_files());

    assert_eq!(
        recover(&file_system, transaction_id),
        RecoveryResult {
            transaction_id: TransactionId(transaction_id.to_string()),
            plan_digest,
            outcome: RecoveryOutcome::Committed,
        }
    );
    assert_eq!(file_system.files(), committed_mixed_files());
    assert_eq!(file_system.artifact_counts(), (0, 0));
}

fn crashed_create_after_apply(transaction_id: &str) -> (TestFileSystem, String, ExactBytesDigest) {
    let file_system = TestFileSystem::new([]);
    let plan = block_on(plan(&file_system, create_request(&["created"]))).unwrap();
    let plan_digest = plan.plan_digest;
    file_system.crash_after_apply_at(/*call*/ 1);
    assert_crashes(|| {
        let _ = block_on(execute(
            &file_system,
            plan,
            TransactionId(transaction_id.to_string()),
            TransactionLimits::default(),
        ));
    });
    (file_system, transaction_id.to_string(), plan_digest)
}

fn recover(file_system: &TestFileSystem, transaction_id: &str) -> RecoveryResult {
    block_on(recover_transaction(
        &file_system.restart(),
        &TestFileSystem::transaction_key(transaction_id),
        TransactionLimits::default(),
    ))
    .unwrap()
}

fn assert_crashes<T>(operation: impl FnOnce() -> T) {
    assert!(catch_unwind(AssertUnwindSafe(operation)).is_err());
}

fn create_request(paths: &[&str]) -> TransactionRequest {
    request(
        paths
            .iter()
            .map(|path| FileMutation::Create {
                path: (*path).to_string(),
                contents: format!("contents-{path}").into_bytes(),
            })
            .collect(),
    )
}

fn mixed_file_system() -> TestFileSystem {
    TestFileSystem::new([
        ("b-delete", b"old-d".as_slice(), 1),
        ("c-move", b"old-m".as_slice(), 2),
        ("e-update", b"old-u".as_slice(), 3),
    ])
}

fn mixed_request() -> TransactionRequest {
    request(vec![
        FileMutation::Create {
            path: "a-create".to_string(),
            contents: b"created".to_vec(),
        },
        FileMutation::Delete {
            path: "b-delete".to_string(),
            expected: expected(b"old-d"),
        },
        FileMutation::Move {
            source: "c-move".to_string(),
            expected: expected(b"old-m"),
            destination: "d-moved".to_string(),
            edits: Vec::new(),
        },
        FileMutation::Update {
            path: "e-update".to_string(),
            expected: expected(b"old-u"),
            edits: vec![FileEdit::ReplaceAll {
                contents: b"updated".to_vec(),
            }],
        },
    ])
}

fn request(mutations: Vec<FileMutation>) -> TransactionRequest {
    TransactionRequest {
        environment_id: "test-environment".to_string(),
        root: PathUri::parse("file:///workspace").unwrap(),
        action: TransactionAction::Commit,
        mutations,
    }
}

fn expected(contents: &[u8]) -> ExpectedFile {
    ExpectedFile {
        exact_digest: ExactBytesDigest::new(contents),
    }
}

fn original_mixed_files() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        ("b-delete".to_string(), b"old-d".to_vec()),
        ("c-move".to_string(), b"old-m".to_vec()),
        ("e-update".to_string(), b"old-u".to_vec()),
    ])
}

fn committed_mixed_files() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        ("a-create".to_string(), b"created".to_vec()),
        ("d-moved".to_string(), b"old-m".to_vec()),
        ("e-update".to_string(), b"updated".to_vec()),
    ])
}
