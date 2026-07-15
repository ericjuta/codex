use thiserror::Error;

use crate::ExecuteError;
use crate::ExecutionResult;
use crate::PlanError;
use crate::TransactionAction;
use crate::TransactionFileSystem;
use crate::TransactionId;
use crate::TransactionLimits;
use crate::TransactionRequest;
use crate::execute;
use crate::plan_with_limits;

/// Failure while planning and executing one commit request in the same executor.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ExecuteRequestError {
    #[error("a preview-only transaction request cannot be executed")]
    PreviewAction,
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    Execute(#[from] ExecuteError),
}

/// Plans and executes one commit without allowing native plan handles to escape.
///
/// `CommitPreviewed` is replanned against current executor state. The planner
/// verifies its expected digest before this function passes the resulting typed
/// plan directly to the recoverable executor.
pub async fn execute_request<F: TransactionFileSystem>(
    file_system: &F,
    request: TransactionRequest,
    transaction_id: TransactionId,
    limits: TransactionLimits,
) -> Result<ExecutionResult, ExecuteRequestError> {
    if request.action == TransactionAction::Preview {
        return Err(ExecuteRequestError::PreviewAction);
    }

    let plan = plan_with_limits(file_system, request, limits).await?;
    Ok(execute(file_system, plan, transaction_id, limits).await?)
}
