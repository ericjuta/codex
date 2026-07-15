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
fn rollback_terminal_states_resume_after_restart() {
    let cases = [
        (8, JournalState::RolledBack, "tx-rolled-back"),
        (9, JournalState::Cleaning, "tx-rollback-cleaning"),
    ];

    for (persist_call, durable_state, transaction_id) in cases {
        let file_system = TestFileSystem::new([]);
        let plan = block_on(plan(&file_system, create_request())).unwrap();
        let plan_digest = plan.plan_digest;
        file_system.fail_apply_at(/*call*/ 1);
        file_system.crash_after_persist_at(persist_call);

        assert_crashes(|| {
            let _ = block_on(execute(
                &file_system,
                plan,
                TransactionId(transaction_id.to_string()),
                TransactionLimits::default(),
            ));
        });
        assert_eq!(file_system.files(), BTreeMap::new());
        assert_eq!(file_system.journals().last().unwrap().state, durable_state);

        assert_eq!(
            recover(&file_system, transaction_id),
            RecoveryResult {
                transaction_id: TransactionId(transaction_id.to_string()),
                plan_digest,
                outcome: RecoveryOutcome::RolledBack,
            },
            "persist call {persist_call}"
        );
        assert_eq!(file_system.files(), BTreeMap::new());
        assert_eq!(file_system.artifact_counts(), (0, 0));
        assert_eq!(
            file_system.journals().last().unwrap().state,
            JournalState::Complete
        );
    }
}

#[test]
fn recovery_required_rollback_resumes_after_restart() {
    let file_system = TestFileSystem::new([]);
    let plan = block_on(plan(&file_system, create_request())).unwrap();
    let plan_digest = plan.plan_digest;
    let transaction_id = "tx-recovery-required";
    file_system.fail_apply_at(/*call*/ 1);
    file_system.fail_restore_at(/*call*/ 1);

    assert_eq!(
        block_on(execute(
            &file_system,
            plan,
            TransactionId(transaction_id.to_string()),
            TransactionLimits::default(),
        )),
        Err(ExecuteError::RecoveryRequired {
            transaction_id: TransactionId(transaction_id.to_string()),
            plan_digest,
            failure: ExecutionFailure::FileSystem(TransactionFileSystemError::Platform {
                operation: "apply guarded",
                reason: "injected failure".to_string(),
            }),
            recovery_failure: ExecutionFailure::FileSystem(TransactionFileSystemError::Platform {
                operation: "restore guarded",
                reason: "injected failure".to_string(),
            },),
        })
    );
    assert_eq!(
        file_system.journals().last().unwrap().state,
        JournalState::RecoveryRequired
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
    assert_eq!(file_system.artifact_counts(), (0, 0));
    assert_eq!(
        file_system.journals().last().unwrap().state,
        JournalState::Complete
    );
}

#[test]
fn invalid_manifest_never_cleans_recovery_evidence() {
    let file_system = TestFileSystem::new([]);
    let plan = block_on(plan(&file_system, create_request())).unwrap();
    let transaction_id = "tx-invalid-manifest";
    file_system.crash_after_persist_at(/*call*/ 4);

    assert_crashes(|| {
        let _ = block_on(execute(
            &file_system,
            plan,
            TransactionId(transaction_id.to_string()),
            TransactionLimits::default(),
        ));
    });
    let artifact_counts = file_system.artifact_counts();
    file_system
        .state
        .lock()
        .unwrap()
        .journals
        .last_mut()
        .unwrap()
        .mutations
        .clear();

    assert_eq!(
        block_on(recover_transaction(
            &file_system.restart(),
            &TestFileSystem::transaction_key(transaction_id),
            TransactionLimits::default(),
        )),
        Err(RecoveryError::Unavailable {
            failure: RecoveryFailure::Journal(JournalError::InvalidField {
                field: "mutations",
                reason: "at least one mutation is required",
            }),
        })
    );
    assert_eq!(file_system.artifact_counts(), artifact_counts);
    assert!(!file_system.events().contains(&TestEvent::Cleaned));
}

fn recover(file_system: &TestFileSystem, transaction_id: &str) -> RecoveryResult {
    block_on(recover_transaction(
        &file_system.restart(),
        &TestFileSystem::transaction_key(transaction_id),
        TransactionLimits::default(),
    ))
    .unwrap()
}

fn create_request() -> TransactionRequest {
    TransactionRequest {
        environment_id: "test-environment".to_string(),
        root: PathUri::parse("file:///workspace").unwrap(),
        action: TransactionAction::Commit,
        mutations: vec![FileMutation::Create {
            path: "created".to_string(),
            contents: b"contents-created".to_vec(),
        }],
    }
}

fn assert_crashes<T>(operation: impl FnOnce() -> T) {
    assert!(catch_unwind(AssertUnwindSafe(operation)).is_err());
}
