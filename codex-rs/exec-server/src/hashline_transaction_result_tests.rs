use super::*;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_CONFLICT_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_MAX_RECOVERY_ATTEMPTS;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_MAX_RESPONSE_BYTES;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_RPC_RESPONSE_OVERHEAD_BYTES;
use codex_exec_server_protocol::HashlineTransactionFailureKind;
use codex_exec_server_protocol::HashlineTransactionRecoveryAttempt;
use codex_hashline_transaction::DurableTransactionKey;
use codex_hashline_transaction::ExactBytesDigest;
use codex_hashline_transaction::RecoveryError;
use codex_hashline_transaction::RecoveryFailure;
use codex_hashline_transaction::RecoveryResult;
use codex_hashline_transaction::TransactionId;
use pretty_assertions::assert_eq;

#[test]
fn bounded_message_preserves_utf8_and_caps_bytes() {
    let message = "é".repeat(HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES);

    let bounded = bounded_message(message);

    assert!(bounded.len() <= HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES);
    assert!(bounded.ends_with("...[truncated]"));
}

#[test]
fn execution_error_maps_conflict_and_bounds_message() {
    let error = ExecuteError::BeforeCommit {
        failure: ExecutionFailure::FileSystem(TransactionFileSystemError::ChangedSincePlanning {
            path: "x".repeat(HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES * 2),
        }),
    };

    let error = execution_error(error);

    assert_eq!(error.code, HASHLINE_TRANSACTION_CONFLICT_ERROR_CODE);
    assert!(error.message.len() <= HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES);
    assert!(error.message.ends_with("...[truncated]"));
    assert_eq!(error.data, None);
}

#[test]
fn recovery_required_error_uses_typed_bounded_data() {
    let transaction_id = TransactionId("txn-1".to_string());
    let plan_digest = ExactBytesDigest::new(b"plan");
    let long_reason = "failure".repeat(HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES);
    let error = ExecuteError::RecoveryRequired {
        transaction_id: transaction_id.clone(),
        plan_digest,
        failure: ExecutionFailure::FileSystem(TransactionFileSystemError::Platform {
            operation: "commit",
            reason: long_reason.clone(),
        }),
        recovery_failure: ExecutionFailure::FileSystem(TransactionFileSystemError::Platform {
            operation: "rollback",
            reason: long_reason,
        }),
    };

    let error = execution_error(error);
    let data: HashlineTransactionRecoveryRequiredData =
        serde_json::from_value(error.data.expect("typed recovery data")).expect("valid data");

    assert_eq!(
        error.code,
        HASHLINE_TRANSACTION_RECOVERY_REQUIRED_ERROR_CODE
    );
    assert_eq!(data.transaction_id, transaction_id);
    assert_eq!(data.plan_digest, plan_digest);
    assert_eq!(
        data.failure.kind,
        HashlineTransactionFailureKind::FileSystem
    );
    assert_eq!(
        data.recovery_failure.kind,
        HashlineTransactionFailureKind::FileSystem
    );
    assert!(data.failure.message.len() <= HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES);
    assert!(data.recovery_failure.message.len() <= HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES);
}

#[test]
fn recovery_response_omits_durable_transaction_keys() {
    let transaction_id = TransactionId("txn-1".to_string());
    let plan_digest = ExactBytesDigest::new(b"plan");
    let response = recovery_response(vec![RecoveryAttempt {
        key: DurableTransactionKey {
            namespace: "native-test".to_string(),
            value: b"secret-key".to_vec(),
        },
        result: Ok(RecoveryResult {
            transaction_id: transaction_id.clone(),
            plan_digest,
            outcome: RecoveryOutcome::Committed,
        }),
    }]);

    assert_eq!(
        response.attempts,
        vec![HashlineTransactionRecoveryAttempt::Committed {
            transaction_id,
            plan_digest,
        }]
    );
    assert!(
        !serde_json::to_string(&response)
            .expect("serialize response")
            .contains("secret-key")
    );
}

#[test]
fn recovery_gate_blocks_failed_attempts_without_exposing_durable_keys() {
    let error = recovery_gate_error(vec![RecoveryAttempt {
        key: DurableTransactionKey {
            namespace: "native-test".to_string(),
            value: b"secret-key".to_vec(),
        },
        result: Err(RecoveryError::Unavailable {
            failure: RecoveryFailure::InvalidTransactionKey {
                reason: "test failure",
            },
        }),
    }])
    .expect("failed recovery must block the next commit");

    assert_eq!(
        error.code,
        HASHLINE_TRANSACTION_RECOVERY_REQUIRED_ERROR_CODE
    );
    assert!(
        !serde_json::to_string(&error.data.expect("typed recovery attempts"))
            .expect("serialize recovery attempts")
            .contains("secret-key")
    );
}

#[test]
fn maximum_recovery_response_stays_inside_the_rpc_response_budget() {
    let attempts = (0..HASHLINE_TRANSACTION_MAX_RECOVERY_ATTEMPTS)
        .map(|index| RecoveryAttempt {
            key: DurableTransactionKey {
                namespace: "native-test".to_string(),
                value: index.to_le_bytes().to_vec(),
            },
            result: Err(RecoveryError::RecoveryRequired {
                transaction_id: TransactionId(format!("txn-{index}")),
                failure: RecoveryFailure::FileSystem(TransactionFileSystemError::Platform {
                    operation: "recover transaction",
                    reason: "failure".repeat(HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES),
                }),
                record_failure: Some(RecoveryFailure::FileSystem(
                    TransactionFileSystemError::Platform {
                        operation: "record recovery failure",
                        reason: "record".repeat(HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES),
                    },
                )),
            }),
        })
        .collect();

    let serialized = serde_json::to_vec(&recovery_response(attempts))
        .expect("serialize maximum recovery response");

    assert!(
        serialized.len() as u64
            <= HASHLINE_TRANSACTION_MAX_RESPONSE_BYTES
                - HASHLINE_TRANSACTION_RPC_RESPONSE_OVERHEAD_BYTES
    );
}
