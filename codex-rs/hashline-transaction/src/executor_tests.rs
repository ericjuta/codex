use std::collections::BTreeMap;

use codex_utils_path_uri::PathUri;
use futures::executor::block_on;
use pretty_assertions::assert_eq;

use super::executor_test_support::TestEvent;
use super::executor_test_support::TestFileSystem;
use super::*;

#[test]
fn mixed_commit_is_ordered_and_leaves_a_terminal_receipt() {
    let file_system = mixed_file_system();
    let plan = block_on(plan(&file_system, mixed_request())).unwrap();
    let plan_digest = plan.plan_digest;
    let transaction_id = TransactionId("tx-success".to_string());

    let result = block_on(execute(
        &file_system,
        plan,
        transaction_id.clone(),
        TransactionLimits::default(),
    ))
    .unwrap();

    assert_eq!(
        result,
        ExecutionResult {
            transaction_id,
            plan_digest,
            outcome: ExecutionOutcome::Committed,
        }
    );
    assert_eq!(
        file_system.files(),
        BTreeMap::from([
            ("a-create".to_string(), b"created".to_vec()),
            ("d-moved".to_string(), b"old-m".to_vec()),
            ("e-update".to_string(), b"updated".to_vec()),
        ])
    );
    assert_eq!(
        file_system.events(),
        vec![
            TestEvent::Locked(vec![
                "a-create".to_string(),
                "b-delete".to_string(),
                "c-move".to_string(),
                "d-moved".to_string(),
                "e-update".to_string(),
            ]),
            TestEvent::Applied("create:a-create".to_string()),
            TestEvent::Applied("remove:b-delete".to_string()),
            TestEvent::Applied("move:c-move->d-moved".to_string()),
            TestEvent::Applied("replace:e-update".to_string()),
            TestEvent::Cleaned,
        ]
    );
    assert_eq!(file_system.artifact_counts(), (0, 0));
    assert_eq!(
        terminal_snapshot(&file_system),
        (JournalState::Complete, vec![MutationProgress::Applied; 4],)
    );
}

#[test]
fn stale_final_revalidation_writes_no_journal_and_changes_nothing() {
    let file_system = TestFileSystem::new([("e-update", b"old-u".as_slice(), 3)]);
    let plan = block_on(plan(
        &file_system,
        request(vec![update("e-update", b"old-u", b"updated")]),
    ))
    .unwrap();
    file_system.external_write("e-update", b"external", /*identity*/ 9);

    let error = block_on(execute(
        &file_system,
        plan,
        TransactionId("tx-stale".to_string()),
        TransactionLimits::default(),
    ))
    .unwrap_err();

    assert_eq!(
        error,
        ExecuteError::BeforeCommit {
            failure: ExecutionFailure::FileSystem(
                TransactionFileSystemError::ChangedSincePlanning {
                    path: "e-update".to_string(),
                },
            ),
        }
    );
    assert_eq!(
        file_system.files(),
        BTreeMap::from([("e-update".to_string(), b"external".to_vec())])
    );
    assert_eq!(file_system.journals(), Vec::new());
    assert_eq!(file_system.artifact_counts(), (0, 0));
    assert_eq!(
        file_system.events(),
        vec![TestEvent::Locked(vec!["e-update".to_string()])]
    );
}

#[test]
fn forward_apply_failure_rolls_back_in_reverse_order() {
    let file_system = mixed_file_system();
    let plan = block_on(plan(&file_system, mixed_request())).unwrap();
    let plan_digest = plan.plan_digest;
    let transaction_id = TransactionId("tx-forward-failure".to_string());
    file_system.fail_apply_at(/*call*/ 4);

    let result = block_on(execute(
        &file_system,
        plan,
        transaction_id.clone(),
        TransactionLimits::default(),
    ))
    .unwrap();

    assert_eq!(
        result,
        ExecutionResult {
            transaction_id,
            plan_digest,
            outcome: ExecutionOutcome::RolledBack {
                failure: injected_failure("apply guarded"),
            },
        }
    );
    assert_eq!(file_system.files(), original_files());
    assert_eq!(
        file_system.events(),
        vec![
            TestEvent::Locked(vec![
                "a-create".to_string(),
                "b-delete".to_string(),
                "c-move".to_string(),
                "d-moved".to_string(),
                "e-update".to_string(),
            ]),
            TestEvent::Applied("create:a-create".to_string()),
            TestEvent::Applied("remove:b-delete".to_string()),
            TestEvent::Applied("move:c-move->d-moved".to_string()),
            TestEvent::Restored("replace:e-update".to_string()),
            TestEvent::Restored("move:c-move->d-moved".to_string()),
            TestEvent::Restored("remove:b-delete".to_string()),
            TestEvent::Restored("create:a-create".to_string()),
            TestEvent::Cleaned,
        ]
    );
    assert_eq!(file_system.artifact_counts(), (0, 0));
    assert_eq!(
        terminal_snapshot(&file_system),
        (
            JournalState::Complete,
            vec![MutationProgress::RolledBack; 4],
        )
    );
}

#[test]
fn rollback_failure_marks_recovery_required_and_retains_evidence() {
    let file_system = mixed_file_system();
    let plan = block_on(plan(&file_system, mixed_request())).unwrap();
    let plan_digest = plan.plan_digest;
    let transaction_id = TransactionId("tx-recovery-required".to_string());
    file_system.fail_apply_at(/*call*/ 3);
    file_system.fail_restore_at(/*call*/ 1);

    let error = block_on(execute(
        &file_system,
        plan,
        transaction_id.clone(),
        TransactionLimits::default(),
    ))
    .unwrap_err();

    assert_eq!(
        error,
        ExecuteError::RecoveryRequired {
            transaction_id,
            plan_digest,
            failure: injected_failure("apply guarded"),
            recovery_failure: injected_failure("restore guarded"),
        }
    );
    assert_eq!(
        file_system.files(),
        BTreeMap::from([
            ("a-create".to_string(), b"created".to_vec()),
            ("c-move".to_string(), b"old-m".to_vec()),
            ("e-update".to_string(), b"old-u".to_vec()),
        ])
    );
    assert_eq!(file_system.artifact_counts(), (3, 3));
    assert_eq!(
        terminal_snapshot(&file_system),
        (
            JournalState::RecoveryRequired,
            vec![
                MutationProgress::Applied,
                MutationProgress::Applied,
                MutationProgress::RollingBack,
                MutationProgress::Pending,
            ],
        )
    );
    assert_eq!(
        file_system.events(),
        vec![
            TestEvent::Locked(vec![
                "a-create".to_string(),
                "b-delete".to_string(),
                "c-move".to_string(),
                "d-moved".to_string(),
                "e-update".to_string(),
            ]),
            TestEvent::Applied("create:a-create".to_string()),
            TestEvent::Applied("remove:b-delete".to_string()),
        ]
    );
}

#[test]
fn journal_failure_before_visible_mutation_completes_clean_rollback() {
    let file_system = TestFileSystem::new([]);
    let plan = block_on(plan(
        &file_system,
        request(vec![create("a-create", b"created")]),
    ))
    .unwrap();
    let plan_digest = plan.plan_digest;
    let transaction_id = TransactionId("tx-journal-before".to_string());
    file_system.fail_persist_at(/*call*/ 4);

    let result = block_on(execute(
        &file_system,
        plan,
        transaction_id.clone(),
        TransactionLimits::default(),
    ))
    .unwrap();

    assert_eq!(
        result,
        ExecutionResult {
            transaction_id,
            plan_digest,
            outcome: ExecutionOutcome::RolledBack {
                failure: injected_failure("persist journal"),
            },
        }
    );
    assert_eq!(file_system.files(), BTreeMap::new());
    assert_eq!(
        file_system.events(),
        vec![
            TestEvent::Locked(vec!["a-create".to_string()]),
            TestEvent::Cleaned,
        ]
    );
    assert_eq!(file_system.artifact_counts(), (0, 0));
    assert_eq!(
        journal_snapshots(&file_system),
        vec![
            (JournalState::Preparing, vec![MutationProgress::Pending]),
            (JournalState::Prepared, vec![MutationProgress::Pending]),
            (JournalState::Committing, vec![MutationProgress::Pending]),
            (JournalState::RollingBack, vec![MutationProgress::Pending]),
            (JournalState::RolledBack, vec![MutationProgress::Pending]),
            (JournalState::Cleaning, vec![MutationProgress::Pending]),
            (JournalState::Complete, vec![MutationProgress::Pending]),
        ]
    );
}

#[test]
fn journal_failure_after_apply_uses_inverse_mutation() {
    let file_system = TestFileSystem::new([]);
    let plan = block_on(plan(
        &file_system,
        request(vec![create("a-create", b"created")]),
    ))
    .unwrap();
    let plan_digest = plan.plan_digest;
    let transaction_id = TransactionId("tx-journal-after".to_string());
    file_system.fail_persist_at(/*call*/ 5);

    let result = block_on(execute(
        &file_system,
        plan,
        transaction_id.clone(),
        TransactionLimits::default(),
    ))
    .unwrap();

    assert_eq!(
        result,
        ExecutionResult {
            transaction_id,
            plan_digest,
            outcome: ExecutionOutcome::RolledBack {
                failure: injected_failure("persist journal"),
            },
        }
    );
    assert_eq!(file_system.files(), BTreeMap::new());
    assert_eq!(
        file_system.events(),
        vec![
            TestEvent::Locked(vec!["a-create".to_string()]),
            TestEvent::Applied("create:a-create".to_string()),
            TestEvent::Restored("create:a-create".to_string()),
            TestEvent::Cleaned,
        ]
    );
    assert_eq!(file_system.artifact_counts(), (0, 0));
    assert_eq!(
        journal_snapshots(&file_system),
        vec![
            (JournalState::Preparing, vec![MutationProgress::Pending]),
            (JournalState::Prepared, vec![MutationProgress::Pending]),
            (JournalState::Committing, vec![MutationProgress::Pending]),
            (JournalState::Committing, vec![MutationProgress::Committing],),
            (
                JournalState::RollingBack,
                vec![MutationProgress::Committing],
            ),
            (
                JournalState::RollingBack,
                vec![MutationProgress::RollingBack],
            ),
            (
                JournalState::RollingBack,
                vec![MutationProgress::RolledBack],
            ),
            (JournalState::RolledBack, vec![MutationProgress::RolledBack],),
            (JournalState::Cleaning, vec![MutationProgress::RolledBack],),
            (JournalState::Complete, vec![MutationProgress::RolledBack],),
        ]
    );
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
        update("e-update", b"old-u", b"updated"),
        FileMutation::Move {
            source: "c-move".to_string(),
            expected: expected(b"old-m"),
            destination: "d-moved".to_string(),
            edits: Vec::new(),
        },
        create("a-create", b"created"),
        FileMutation::Delete {
            path: "b-delete".to_string(),
            expected: expected(b"old-d"),
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

fn create(path: &str, contents: &[u8]) -> FileMutation {
    FileMutation::Create {
        path: path.to_string(),
        contents: contents.to_vec(),
    }
}

fn update(path: &str, before: &[u8], after: &[u8]) -> FileMutation {
    FileMutation::Update {
        path: path.to_string(),
        expected: expected(before),
        edits: vec![FileEdit::ReplaceAll {
            contents: after.to_vec(),
        }],
    }
}

fn expected(contents: &[u8]) -> ExpectedFile {
    ExpectedFile {
        exact_digest: ExactBytesDigest::new(contents),
    }
}

fn original_files() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        ("b-delete".to_string(), b"old-d".to_vec()),
        ("c-move".to_string(), b"old-m".to_vec()),
        ("e-update".to_string(), b"old-u".to_vec()),
    ])
}

fn injected_failure(operation: &'static str) -> ExecutionFailure {
    ExecutionFailure::FileSystem(TransactionFileSystemError::Platform {
        operation,
        reason: "injected failure".to_string(),
    })
}

fn terminal_snapshot(file_system: &TestFileSystem) -> (JournalState, Vec<MutationProgress>) {
    journal_snapshots(file_system).pop().unwrap()
}

fn journal_snapshots(file_system: &TestFileSystem) -> Vec<(JournalState, Vec<MutationProgress>)> {
    file_system
        .journals()
        .into_iter()
        .map(|record| {
            (
                record.state,
                record
                    .mutations
                    .into_iter()
                    .map(|mutation| mutation.progress)
                    .collect(),
            )
        })
        .collect()
}
