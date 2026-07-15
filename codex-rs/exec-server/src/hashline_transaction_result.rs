use codex_exec_server_protocol::HASHLINE_TRANSACTION_CONFLICT_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_RECOVERY_REQUIRED_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_UNSUPPORTED_ERROR_CODE;
use codex_exec_server_protocol::HashlineTransactionExecuteResponse;
use codex_exec_server_protocol::HashlineTransactionExecutionOutcome;
use codex_exec_server_protocol::HashlineTransactionFailure;
use codex_exec_server_protocol::HashlineTransactionFailureKind;
use codex_exec_server_protocol::HashlineTransactionRecoverResponse;
use codex_exec_server_protocol::HashlineTransactionRecoveryAttempt;
use codex_exec_server_protocol::HashlineTransactionRecoveryRequiredData;
use codex_exec_server_protocol::JSONRPCErrorError;
use codex_hashline_transaction::ExecuteError;
use codex_hashline_transaction::ExecutionFailure;
use codex_hashline_transaction::ExecutionOutcome;
use codex_hashline_transaction::ExecutionResult;
use codex_hashline_transaction::PlanPreview;
use codex_hashline_transaction::RecoveryAttempt;
use codex_hashline_transaction::RecoveryError;
use codex_hashline_transaction::RecoveryFailure;
use codex_hashline_transaction::RecoveryOutcome;
use codex_hashline_transaction::TransactionFileSystemError;

const RECOVERY_FAILURE_MESSAGE_BYTES: usize = 1024;

pub(crate) fn execution_response(
    preview: PlanPreview,
    result: ExecutionResult,
) -> HashlineTransactionExecuteResponse {
    let outcome = match result.outcome {
        ExecutionOutcome::Committed => HashlineTransactionExecutionOutcome::Committed,
        ExecutionOutcome::RolledBack { failure } => {
            HashlineTransactionExecutionOutcome::RolledBack {
                failure: execution_failure(&failure),
            }
        }
    };
    HashlineTransactionExecuteResponse {
        preview,
        transaction_id: result.transaction_id,
        outcome,
    }
}

pub(crate) fn execution_error(error: ExecuteError) -> JSONRPCErrorError {
    match error {
        ExecuteError::PreviewPlan => rpc_error(
            HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE,
            error.to_string(),
            None,
        ),
        ExecuteError::BeforeCommit { failure } => {
            rpc_error(execution_failure_code(&failure), failure.to_string(), None)
        }
        ExecuteError::RecoveryRequired {
            transaction_id,
            plan_digest,
            failure,
            recovery_failure,
        } => {
            let data = HashlineTransactionRecoveryRequiredData {
                transaction_id,
                plan_digest,
                failure: execution_failure(&failure),
                recovery_failure: execution_failure(&recovery_failure),
            };
            rpc_error(
                HASHLINE_TRANSACTION_RECOVERY_REQUIRED_ERROR_CODE,
                "transaction requires executor recovery".to_string(),
                serde_json::to_value(data).ok(),
            )
        }
    }
}

pub(crate) fn recovery_response(
    attempts: Vec<RecoveryAttempt>,
) -> HashlineTransactionRecoverResponse {
    let attempts = attempts
        .into_iter()
        .map(|attempt| match attempt.result {
            Ok(result) => match result.outcome {
                RecoveryOutcome::Committed => HashlineTransactionRecoveryAttempt::Committed {
                    transaction_id: result.transaction_id,
                    plan_digest: result.plan_digest,
                },
                RecoveryOutcome::RolledBack => HashlineTransactionRecoveryAttempt::RolledBack {
                    transaction_id: result.transaction_id,
                    plan_digest: result.plan_digest,
                },
            },
            Err(RecoveryError::Unavailable { failure }) => {
                HashlineTransactionRecoveryAttempt::Unavailable {
                    failure: recovery_failure(&failure),
                }
            }
            Err(RecoveryError::RecoveryRequired {
                transaction_id,
                failure,
                record_failure,
            }) => HashlineTransactionRecoveryAttempt::RecoveryRequired {
                transaction_id,
                failure: recovery_failure(&failure),
                record_failure: record_failure.as_ref().map(recovery_failure),
            },
        })
        .collect();
    HashlineTransactionRecoverResponse { attempts }
}

pub(crate) fn recovery_gate_error(attempts: Vec<RecoveryAttempt>) -> Option<JSONRPCErrorError> {
    if attempts.iter().all(|attempt| attempt.result.is_ok()) {
        return None;
    }
    Some(rpc_error(
        HASHLINE_TRANSACTION_RECOVERY_REQUIRED_ERROR_CODE,
        "unfinished transaction requires executor recovery".to_string(),
        serde_json::to_value(recovery_response(attempts)).ok(),
    ))
}

pub(crate) fn recovery_scan_error(failure: RecoveryFailure) -> JSONRPCErrorError {
    let code = match &failure {
        RecoveryFailure::FileSystem(error) => file_system_error_code(error),
        RecoveryFailure::Journal(_)
        | RecoveryFailure::ScanLimit { .. }
        | RecoveryFailure::InvalidTransactionKey { .. }
        | RecoveryFailure::TransactionKeyMismatch
        | RecoveryFailure::EnvironmentMismatch
        | RecoveryFailure::ManifestMismatch
        | RecoveryFailure::RootIdentityMismatch => HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE,
    };
    rpc_error(code, failure.to_string(), None)
}

fn execution_failure(failure: &ExecutionFailure) -> HashlineTransactionFailure {
    let kind = match failure {
        ExecutionFailure::FileSystem(_) => HashlineTransactionFailureKind::FileSystem,
        ExecutionFailure::Journal(_) => HashlineTransactionFailureKind::Journal,
    };
    HashlineTransactionFailure {
        kind,
        message: bounded_message(failure.to_string()),
    }
}

fn recovery_failure(failure: &RecoveryFailure) -> HashlineTransactionFailure {
    let kind = match failure {
        RecoveryFailure::FileSystem(_) => HashlineTransactionFailureKind::FileSystem,
        RecoveryFailure::Journal(_) => HashlineTransactionFailureKind::Journal,
        RecoveryFailure::ScanLimit { .. }
        | RecoveryFailure::InvalidTransactionKey { .. }
        | RecoveryFailure::TransactionKeyMismatch
        | RecoveryFailure::EnvironmentMismatch
        | RecoveryFailure::ManifestMismatch
        | RecoveryFailure::RootIdentityMismatch => HashlineTransactionFailureKind::Recovery,
    };
    HashlineTransactionFailure {
        kind,
        message: bounded_message_to(failure.to_string(), RECOVERY_FAILURE_MESSAGE_BYTES),
    }
}

fn execution_failure_code(failure: &ExecutionFailure) -> i64 {
    match failure {
        ExecutionFailure::FileSystem(error) => file_system_error_code(error),
        ExecutionFailure::Journal(_) => HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE,
    }
}

fn file_system_error_code(error: &TransactionFileSystemError) -> i64 {
    match error {
        TransactionFileSystemError::Unsupported { .. } => {
            HASHLINE_TRANSACTION_UNSUPPORTED_ERROR_CODE
        }
        TransactionFileSystemError::InvalidRoot { .. }
        | TransactionFileSystemError::InvalidModelPath { .. }
        | TransactionFileSystemError::SymbolicLink { .. } => {
            HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE
        }
        TransactionFileSystemError::ChangedSincePlanning { .. } => {
            HASHLINE_TRANSACTION_CONFLICT_ERROR_CODE
        }
        TransactionFileSystemError::Platform { .. } => HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE,
    }
}

fn rpc_error(code: i64, message: String, data: Option<serde_json::Value>) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code,
        data,
        message: bounded_message(message),
    }
}

fn bounded_message(message: String) -> String {
    bounded_message_to(message, HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES)
}

fn bounded_message_to(message: String, max_bytes: usize) -> String {
    const TRUNCATION_MARKER: &str = "...[truncated]";
    if message.len() <= max_bytes {
        return message;
    }

    let mut end = max_bytes - TRUNCATION_MARKER.len();
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = message[..end].to_string();
    bounded.push_str(TRUNCATION_MARKER);
    bounded
}

#[cfg(test)]
#[path = "hashline_transaction_result_tests.rs"]
mod tests;
