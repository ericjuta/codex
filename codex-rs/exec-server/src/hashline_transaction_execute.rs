use codex_exec_server_protocol::HASHLINE_TRANSACTION_CONFLICT_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_MAX_RECOVERY_ATTEMPTS;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_MAX_RESPONSE_BYTES;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_RECOVERY_REQUIRED_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_RPC_RESPONSE_OVERHEAD_BYTES;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_UNSUPPORTED_ERROR_CODE;
use codex_exec_server_protocol::JSONRPCErrorError;
use codex_hashline_transaction::PlanError;
use codex_hashline_transaction::RecoveryScanLimit;
use codex_hashline_transaction::TransactionAction;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_hashline_transaction::TransactionId;
use codex_hashline_transaction::TransactionLimits;
use codex_hashline_transaction::TransactionRequest;
use codex_hashline_transaction::build_preview;
use codex_hashline_transaction::execute;
use codex_hashline_transaction::plan_with_limits;
use codex_hashline_transaction::recover_pending;
use uuid::Uuid;

use crate::ExecServerRuntimePaths;
use crate::FileSystemSandboxContext;
use crate::NativeTransactionFileSystem;
use crate::fs_helper::FsHelperRequest;
use crate::fs_sandbox::FileSystemSandboxRunner;
use crate::hashline_transaction_result::execution_error;
use crate::hashline_transaction_result::execution_response;
use crate::hashline_transaction_result::recovery_gate_error;
use crate::hashline_transaction_result::recovery_scan_error;
use crate::protocol::HashlineTransactionExecuteAction;
use crate::protocol::HashlineTransactionExecuteParams;
use crate::protocol::HashlineTransactionExecuteResponse;

/// Owns executor-local Hashline transaction commits and their sandbox routing.
#[derive(Clone, Debug)]
pub(crate) struct HashlineTransactionExecutor {
    sandbox_runner: FileSystemSandboxRunner,
}

impl HashlineTransactionExecutor {
    pub(crate) fn new(runtime_paths: ExecServerRuntimePaths) -> Self {
        Self {
            sandbox_runner: FileSystemSandboxRunner::new(runtime_paths),
        }
    }

    pub(crate) async fn execute(
        &self,
        mut params: HashlineTransactionExecuteParams,
    ) -> Result<HashlineTransactionExecuteResponse, JSONRPCErrorError> {
        if params
            .sandbox
            .as_ref()
            .is_some_and(FileSystemSandboxContext::should_run_in_sandbox)
        {
            let sandbox = params.sandbox.take().ok_or_else(|| {
                executor_rpc_error(
                    HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE,
                    "Hashline transaction sandbox context disappeared".to_string(),
                )
            })?;
            let payload = self
                .sandbox_runner
                .run(
                    &sandbox,
                    FsHelperRequest::HashlineTransactionExecute(params),
                )
                .await
                .map_err(normalize_executor_rpc_error)?;
            return payload
                .expect_hashline_transaction_execute()
                .map_err(normalize_executor_rpc_error);
        }

        execute_direct(params).await
    }
}

/// Executes a transaction in the current executor, rejecting sandboxed direct calls.
pub(crate) async fn execute_direct(
    params: HashlineTransactionExecuteParams,
) -> Result<HashlineTransactionExecuteResponse, JSONRPCErrorError> {
    if params
        .sandbox
        .as_ref()
        .is_some_and(FileSystemSandboxContext::should_run_in_sandbox)
    {
        return Err(executor_rpc_error(
            HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE,
            "restricted Hashline transaction execution must run in the filesystem sandbox"
                .to_string(),
        ));
    }

    let mut limits = TransactionLimits::default();
    limits.max_response_bytes = limits.max_response_bytes.min(
        HASHLINE_TRANSACTION_MAX_RESPONSE_BYTES
            .saturating_sub(HASHLINE_TRANSACTION_RPC_RESPONSE_OVERHEAD_BYTES),
    );
    let action = match params.action {
        HashlineTransactionExecuteAction::Commit => TransactionAction::Commit,
        HashlineTransactionExecuteAction::CommitPreviewed {
            expected_plan_digest,
        } => TransactionAction::CommitPreviewed {
            expected_plan_digest,
        },
    };
    let file_system =
        NativeTransactionFileSystem::new(params.environment_id.clone(), params.root.clone());
    let recovery_attempts = recover_pending(
        &file_system,
        RecoveryScanLimit {
            max_transactions: HASHLINE_TRANSACTION_MAX_RECOVERY_ATTEMPTS,
        },
        limits,
    )
    .await
    .map_err(recovery_scan_error)?;
    if let Some(error) = recovery_gate_error(recovery_attempts) {
        return Err(error);
    }
    let request = TransactionRequest {
        environment_id: params.environment_id,
        root: params.root,
        action,
        mutations: params.mutations,
    };
    let plan = plan_with_limits(&file_system, request, limits)
        .await
        .map_err(map_plan_error)?;
    let preview = build_preview(&plan, limits).map_err(map_plan_error)?;
    let transaction_id = TransactionId(Uuid::new_v4().simple().to_string());
    let result = execute(&file_system, plan, transaction_id, limits)
        .await
        .map_err(execution_error)?;
    Ok(execution_response(preview, result))
}

fn map_plan_error(error: PlanError) -> JSONRPCErrorError {
    let code = match &error {
        PlanError::FileSystem(file_system_error) => file_system_error_code(file_system_error),
        PlanError::ExpectedAbsent { .. }
        | PlanError::ExpectedExistingFile { .. }
        | PlanError::Stale { .. }
        | PlanError::PlanDigestMismatch { .. } => HASHLINE_TRANSACTION_CONFLICT_ERROR_CODE,
        PlanError::PreviewSerialization { .. } => HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE,
        PlanError::Empty
        | PlanError::Limit { .. }
        | PlanError::PathConflict { .. }
        | PlanError::UnsupportedKind { .. }
        | PlanError::HardLink { .. }
        | PlanError::InvalidEdits { .. }
        | PlanError::InvalidUtf8 { .. }
        | PlanError::InvalidAnchor { .. }
        | PlanError::InvalidEditText { .. } => HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE,
    };
    executor_rpc_error(code, error.to_string())
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

fn normalize_executor_rpc_error(error: JSONRPCErrorError) -> JSONRPCErrorError {
    let JSONRPCErrorError {
        code: source_code,
        data,
        message,
    } = error;
    let code = match source_code {
        HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE
        | HASHLINE_TRANSACTION_UNSUPPORTED_ERROR_CODE
        | HASHLINE_TRANSACTION_CONFLICT_ERROR_CODE
        | HASHLINE_TRANSACTION_RECOVERY_REQUIRED_ERROR_CODE
        | HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE => source_code,
        -32600 => HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE,
        _ => HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE,
    };
    let mut error = executor_rpc_error(code, message);
    if code == source_code {
        error.data = data;
    }
    error
}

#[cfg(all(test, target_os = "linux"))]
#[path = "hashline_transaction_execute_tests.rs"]
mod tests;

fn executor_rpc_error(code: i64, mut message: String) -> JSONRPCErrorError {
    if message.len() > HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES {
        const MARKER: &str = "...";
        let mut end = HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES - MARKER.len();
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push_str(MARKER);
    }
    JSONRPCErrorError {
        code,
        data: None,
        message,
    }
}
