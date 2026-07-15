use codex_exec_server_protocol::JSONRPCErrorError;
use codex_hashline_transaction::FileMutation;
use codex_utils_path_uri::PathUri;

use super::Environment;
use crate::ExecServerError;
use crate::FileSystemSandboxContext;
use crate::hashline_transaction_execute::HashlineTransactionExecutor;
use crate::hashline_transaction_execute::execute_direct;
use crate::hashline_transaction_plan::HashlineTransactionPlanner;
use crate::hashline_transaction_plan::plan_direct;
use crate::hashline_transaction_recover::HashlineTransactionRecoverer;
use crate::hashline_transaction_recover::recover_direct;
use crate::protocol::HashlineTransactionExecuteAction;
use crate::protocol::HashlineTransactionExecuteParams;
use crate::protocol::HashlineTransactionExecuteResponse;
use crate::protocol::HashlineTransactionPlanParams;
use crate::protocol::HashlineTransactionPlanResponse;
use crate::protocol::HashlineTransactionRecoverParams;
use crate::protocol::HashlineTransactionRecoverResponse;

/// Caller-provided inputs for a preview-only transaction in a selected environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HashlineTransactionPlanRequest {
    pub root: PathUri,
    pub mutations: Vec<FileMutation>,
    pub sandbox: Option<FileSystemSandboxContext>,
}

/// Caller-provided inputs for a committing transaction in a selected environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HashlineTransactionExecuteRequest {
    pub root: PathUri,
    pub mutations: Vec<FileMutation>,
    pub action: HashlineTransactionExecuteAction,
    pub sandbox: Option<FileSystemSandboxContext>,
}

/// Caller-provided inputs for recovery in a selected environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HashlineTransactionRecoverRequest {
    pub root: PathUri,
    pub sandbox: Option<FileSystemSandboxContext>,
}

impl Environment {
    /// Plans a no-write Hashline transaction in the selected environment.
    pub async fn plan_hashline_transaction(
        &self,
        request: HashlineTransactionPlanRequest,
    ) -> Result<HashlineTransactionPlanResponse, ExecServerError> {
        let params = HashlineTransactionPlanParams {
            environment_id: self.environment_id.clone(),
            root: request.root,
            mutations: request.mutations,
            sandbox: request.sandbox,
        };
        if let Some(client) = &self.remote_client {
            return client.hashline_transaction_plan(params).await;
        }

        match &self.local_runtime_paths {
            Some(runtime_paths) => HashlineTransactionPlanner::new(runtime_paths.clone())
                .plan(params)
                .await
                .map_err(map_server_error),
            None => plan_direct(params).await.map_err(map_server_error),
        }
    }

    /// Commits a Hashline transaction in the selected environment.
    pub async fn execute_hashline_transaction(
        &self,
        request: HashlineTransactionExecuteRequest,
    ) -> Result<HashlineTransactionExecuteResponse, ExecServerError> {
        let params = HashlineTransactionExecuteParams {
            environment_id: self.environment_id.clone(),
            root: request.root,
            mutations: request.mutations,
            action: request.action,
            sandbox: request.sandbox,
        };
        if let Some(client) = &self.remote_client {
            return client.hashline_transaction_execute(params).await;
        }
        match &self.local_runtime_paths {
            Some(runtime_paths) => HashlineTransactionExecutor::new(runtime_paths.clone())
                .execute(params)
                .await
                .map_err(map_server_error),
            None => execute_direct(params).await.map_err(map_server_error),
        }
    }

    /// Recovers pending Hashline transactions in the selected environment.
    pub async fn recover_hashline_transactions(
        &self,
        request: HashlineTransactionRecoverRequest,
    ) -> Result<HashlineTransactionRecoverResponse, ExecServerError> {
        let params = HashlineTransactionRecoverParams {
            environment_id: self.environment_id.clone(),
            root: request.root,
            sandbox: request.sandbox,
        };
        if let Some(client) = &self.remote_client {
            return client.hashline_transaction_recover(params).await;
        }
        match &self.local_runtime_paths {
            Some(runtime_paths) => HashlineTransactionRecoverer::new(runtime_paths.clone())
                .recover(params)
                .await
                .map_err(map_server_error),
            None => recover_direct(params).await.map_err(map_server_error),
        }
    }
}

fn map_server_error(error: JSONRPCErrorError) -> ExecServerError {
    ExecServerError::Server {
        code: error.code,
        message: error.message,
    }
}
