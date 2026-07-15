use codex_exec_server_protocol::HASHLINE_TRANSACTION_CONFLICT_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_MAX_RESPONSE_BYTES;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_RPC_RESPONSE_OVERHEAD_BYTES;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_UNSUPPORTED_ERROR_CODE;
use codex_exec_server_protocol::JSONRPCErrorError;
use codex_hashline_transaction::PlanError;
use codex_hashline_transaction::TransactionAction;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_hashline_transaction::TransactionLimits;
use codex_hashline_transaction::TransactionRequest;
use codex_hashline_transaction::build_preview;
use codex_hashline_transaction::plan_with_limits;

use crate::ExecServerRuntimePaths;
use crate::FileSystemSandboxContext;
use crate::NativePlanningFileSystem;
use crate::fs_helper::FsHelperRequest;
use crate::fs_sandbox::FileSystemSandboxRunner;
use crate::protocol::HashlineTransactionPlanParams;
use crate::protocol::HashlineTransactionPlanResponse;

#[derive(Clone, Debug)]
pub(crate) struct HashlineTransactionPlanner {
    sandbox_runner: FileSystemSandboxRunner,
}

impl HashlineTransactionPlanner {
    pub(crate) fn new(runtime_paths: ExecServerRuntimePaths) -> Self {
        Self {
            sandbox_runner: FileSystemSandboxRunner::new(runtime_paths),
        }
    }

    pub(crate) async fn plan(
        &self,
        mut params: HashlineTransactionPlanParams,
    ) -> Result<HashlineTransactionPlanResponse, JSONRPCErrorError> {
        if params
            .sandbox
            .as_ref()
            .is_some_and(FileSystemSandboxContext::should_run_in_sandbox)
        {
            let sandbox = params.sandbox.take().ok_or_else(|| {
                plan_rpc_error(
                    HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE,
                    "Hashline transaction sandbox context disappeared".to_string(),
                )
            })?;
            let payload = self
                .sandbox_runner
                .run(&sandbox, FsHelperRequest::HashlineTransactionPlan(params))
                .await
                .map_err(normalize_plan_rpc_error)?;
            return payload
                .expect_hashline_transaction_plan()
                .map_err(normalize_plan_rpc_error);
        }

        plan_direct(params).await
    }
}

pub(crate) async fn plan_direct(
    params: HashlineTransactionPlanParams,
) -> Result<HashlineTransactionPlanResponse, JSONRPCErrorError> {
    if params
        .sandbox
        .as_ref()
        .is_some_and(FileSystemSandboxContext::should_run_in_sandbox)
    {
        return Err(plan_rpc_error(
            HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE,
            "restricted Hashline transaction planning must run in the filesystem sandbox"
                .to_string(),
        ));
    }

    let mut limits = TransactionLimits::default();
    limits.max_response_bytes = limits.max_response_bytes.min(
        HASHLINE_TRANSACTION_MAX_RESPONSE_BYTES
            .saturating_sub(HASHLINE_TRANSACTION_RPC_RESPONSE_OVERHEAD_BYTES),
    );
    let request = TransactionRequest {
        environment_id: params.environment_id,
        root: params.root,
        action: TransactionAction::Preview,
        mutations: params.mutations,
    };
    let plan = plan_with_limits(&NativePlanningFileSystem, request, limits)
        .await
        .map_err(map_plan_error)?;
    let preview = build_preview(&plan, limits).map_err(map_plan_error)?;
    Ok(HashlineTransactionPlanResponse { preview })
}

fn map_plan_error(error: PlanError) -> JSONRPCErrorError {
    let code = match &error {
        PlanError::FileSystem(file_system_error) => match file_system_error {
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
        },
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
    plan_rpc_error(code, error.to_string())
}

fn plan_rpc_error(code: i64, message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code,
        data: None,
        message: bounded_error_message(message),
    }
}

fn normalize_plan_rpc_error(error: JSONRPCErrorError) -> JSONRPCErrorError {
    let code = match error.code {
        HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE
        | HASHLINE_TRANSACTION_UNSUPPORTED_ERROR_CODE
        | HASHLINE_TRANSACTION_CONFLICT_ERROR_CODE
        | HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE => error.code,
        -32600 => HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE,
        _ => HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE,
    };
    plan_rpc_error(code, error.message)
}

fn bounded_error_message(mut message: String) -> String {
    if message.len() <= HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES {
        return message;
    }

    const TRUNCATION_MARKER: &str = "...";
    let mut end = HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES - TRUNCATION_MARKER.len();
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.truncate(end);
    message.push_str(TRUNCATION_MARKER);
    message
}

#[cfg(test)]
#[path = "hashline_transaction_plan_tests.rs"]
mod tests;
