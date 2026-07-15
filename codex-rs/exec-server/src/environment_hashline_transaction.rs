use codex_exec_server_protocol::JSONRPCErrorError;
use codex_hashline_transaction::FileMutation;
use codex_utils_path_uri::PathUri;

use super::Environment;
use crate::ExecServerError;
use crate::FileSystemSandboxContext;
use crate::hashline_transaction_plan::HashlineTransactionPlanner;
use crate::hashline_transaction_plan::plan_direct;
use crate::protocol::HashlineTransactionPlanParams;
use crate::protocol::HashlineTransactionPlanResponse;

/// Caller-provided inputs for a preview-only transaction in a selected environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HashlineTransactionPlanRequest {
    pub root: PathUri,
    pub mutations: Vec<FileMutation>,
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
}

fn map_server_error(error: JSONRPCErrorError) -> ExecServerError {
    ExecServerError::Server {
        code: error.code,
        message: error.message,
    }
}
