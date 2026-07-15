use codex_exec_server_protocol::HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_MAX_RECOVERY_ATTEMPTS;
use codex_exec_server_protocol::JSONRPCErrorError;
use codex_hashline_transaction::RecoveryScanLimit;
use codex_hashline_transaction::TransactionLimits;
use codex_hashline_transaction::recover_pending;

use crate::ExecServerRuntimePaths;
use crate::FileSystemSandboxContext;
use crate::NativeTransactionFileSystem;
use crate::fs_helper::FsHelperRequest;
use crate::fs_sandbox::FileSystemSandboxRunner;
use crate::hashline_transaction_result::recovery_response;
use crate::hashline_transaction_result::recovery_scan_error;
use crate::protocol::HashlineTransactionRecoverParams;
use crate::protocol::HashlineTransactionRecoverResponse;

/// Owns explicit root-scoped Hashline transaction recovery and sandbox routing.
#[derive(Clone, Debug)]
pub(crate) struct HashlineTransactionRecoverer {
    sandbox_runner: FileSystemSandboxRunner,
}

impl HashlineTransactionRecoverer {
    pub(crate) fn new(runtime_paths: ExecServerRuntimePaths) -> Self {
        Self {
            sandbox_runner: FileSystemSandboxRunner::new(runtime_paths),
        }
    }

    pub(crate) async fn recover(
        &self,
        mut params: HashlineTransactionRecoverParams,
    ) -> Result<HashlineTransactionRecoverResponse, JSONRPCErrorError> {
        if params
            .sandbox
            .as_ref()
            .is_some_and(FileSystemSandboxContext::should_run_in_sandbox)
        {
            let sandbox = params.sandbox.take().ok_or_else(|| {
                recovery_rpc_error("Hashline transaction sandbox context disappeared".to_string())
            })?;
            let payload = self
                .sandbox_runner
                .run(
                    &sandbox,
                    FsHelperRequest::HashlineTransactionRecover(params),
                )
                .await
                .map_err(normalize_recovery_rpc_error)?;
            return payload
                .expect_hashline_transaction_recover()
                .map_err(normalize_recovery_rpc_error);
        }

        recover_direct(params).await
    }
}

/// Recovers pending transactions from one root in the current executor.
pub(crate) async fn recover_direct(
    params: HashlineTransactionRecoverParams,
) -> Result<HashlineTransactionRecoverResponse, JSONRPCErrorError> {
    if params
        .sandbox
        .as_ref()
        .is_some_and(FileSystemSandboxContext::should_run_in_sandbox)
    {
        return Err(JSONRPCErrorError {
            code: HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE,
            data: None,
            message: "restricted Hashline transaction recovery must run in the filesystem sandbox"
                .to_string(),
        });
    }

    let file_system = NativeTransactionFileSystem::new(params.environment_id, params.root);
    let attempts = recover_pending(
        &file_system,
        RecoveryScanLimit {
            max_transactions: HASHLINE_TRANSACTION_MAX_RECOVERY_ATTEMPTS,
        },
        TransactionLimits::default(),
    )
    .await
    .map_err(recovery_scan_error)?;
    Ok(recovery_response(attempts))
}

fn normalize_recovery_rpc_error(error: JSONRPCErrorError) -> JSONRPCErrorError {
    if error.code == HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE {
        error
    } else {
        recovery_rpc_error(error.message)
    }
}

fn recovery_rpc_error(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE,
        data: None,
        message,
    }
}
