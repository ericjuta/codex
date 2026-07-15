use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;

use codex_utils_path_uri::PathUri;
use futures::executor::block_on;
use pretty_assertions::assert_eq;

use super::executor_test_support::TestFileSystem;
use super::*;

#[test]
fn invalid_and_duplicate_scan_entries_do_not_starve_valid_recovery() {
    let file_system = TestFileSystem::new([]);
    let plan_digest = crash_create(&file_system, "created", "tx-valid", /*persist_call*/ 4);
    let valid = TestFileSystem::transaction_key("tx-valid");
    let invalid = DurableTransactionKey {
        namespace: String::new(),
        value: vec![1],
    };
    file_system.set_pending_recovery(vec![invalid.clone(), valid.clone(), valid.clone()]);

    assert_eq!(
        block_on(recover_pending(
            &file_system.restart(),
            RecoveryScanLimit {
                max_transactions: 3,
            },
            TransactionLimits::default(),
        ))
        .unwrap(),
        vec![
            RecoveryAttempt {
                key: invalid,
                result: Err(RecoveryError::Unavailable {
                    failure: RecoveryFailure::InvalidTransactionKey {
                        reason: "namespace and value must not be empty",
                    },
                }),
            },
            RecoveryAttempt {
                key: valid.clone(),
                result: Ok(RecoveryResult {
                    transaction_id: TransactionId("tx-valid".to_string()),
                    plan_digest,
                    outcome: RecoveryOutcome::RolledBack,
                }),
            },
            RecoveryAttempt {
                key: valid,
                result: Err(RecoveryError::Unavailable {
                    failure: RecoveryFailure::InvalidTransactionKey {
                        reason: "recovery scan returned a duplicate key",
                    },
                }),
            },
        ]
    );
    assert_eq!(file_system.artifact_counts(), (0, 0));
}

#[test]
fn cleanup_is_scoped_to_the_recovered_transaction() {
    let file_system = TestFileSystem::new([]);
    crash_create(&file_system, "first", "tx-first", /*persist_call*/ 4);
    crash_create(&file_system, "second", "tx-second", /*persist_call*/ 8);
    assert_eq!(file_system.artifact_counts(), (2, 0));

    block_on(recover_transaction(
        &file_system.restart(),
        &TestFileSystem::transaction_key("tx-first"),
        TransactionLimits::default(),
    ))
    .unwrap();
    assert_eq!(file_system.artifact_counts(), (1, 0));

    block_on(recover_transaction(
        &file_system.restart(),
        &TestFileSystem::transaction_key("tx-second"),
        TransactionLimits::default(),
    ))
    .unwrap();
    assert_eq!(file_system.artifact_counts(), (0, 0));
}

fn crash_create(
    file_system: &TestFileSystem,
    path: &str,
    transaction_id: &str,
    persist_call: usize,
) -> ExactBytesDigest {
    let plan = block_on(plan(file_system, request(path))).unwrap();
    let plan_digest = plan.plan_digest;
    file_system.crash_after_persist_at(persist_call);
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _ = block_on(execute(
                file_system,
                plan,
                TransactionId(transaction_id.to_string()),
                TransactionLimits::default(),
            ));
        }))
        .is_err()
    );
    plan_digest
}

fn request(path: &str) -> TransactionRequest {
    TransactionRequest {
        environment_id: "test-environment".to_string(),
        root: PathUri::parse("file:///workspace").unwrap(),
        action: TransactionAction::Commit,
        mutations: vec![FileMutation::Create {
            path: path.to_string(),
            contents: format!("contents-{path}").into_bytes(),
        }],
    }
}
