use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::CellHost;
use super::CellToolCall;
use crate::TaskFailureHandler;
use crate::runtime::RuntimeCommand;

const NOTIFICATION_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
pub(super) enum CallbackCompletion {
    DrainNotifications,
    Cancel,
}

pub(super) fn spawn_notification<H: CellHost>(
    tasks: &mut JoinSet<()>,
    host: Arc<H>,
    call_id: String,
    text: String,
    max_output_tokens: Option<usize>,
    cancellation_token: CancellationToken,
    task_failure_handler: Option<TaskFailureHandler>,
) {
    tasks.spawn(async move {
        let callback = AssertUnwindSafe(async move {
            host.notify(call_id, text, max_output_tokens, cancellation_token)
                .await
        })
        .catch_unwind()
        .await;
        match callback {
            Ok(Ok(())) => {}
            Ok(Err(err)) => warn!("failed to deliver code mode notification: {err}"),
            Err(_) => report_task_failure(
                task_failure_handler.as_ref(),
                "code mode notification task panicked".to_string(),
            ),
        }
    });
}

pub(super) fn spawn_tool<H: CellHost>(
    tasks: &mut JoinSet<()>,
    host: Arc<H>,
    invocation: CellToolCall,
    runtime_tx: std::sync::mpsc::Sender<RuntimeCommand>,
    cancellation_token: CancellationToken,
    task_failure_handler: Option<TaskFailureHandler>,
) {
    tasks.spawn(async move {
        let id = invocation.id.clone();
        let callback =
            AssertUnwindSafe(async move { host.invoke_tool(invocation, cancellation_token).await })
                .catch_unwind()
                .await;
        let (command, failure_reason) = match callback {
            Ok(Ok(result)) => (RuntimeCommand::ToolResponse { id, result }, None),
            Ok(Err(error_text)) => (RuntimeCommand::ToolError { id, error_text }, None),
            Err(_) => {
                let failure_reason = "code mode tool task panicked".to_string();
                (
                    RuntimeCommand::ToolError {
                        id,
                        error_text: failure_reason.clone(),
                    },
                    Some(failure_reason),
                )
            }
        };
        let _ = runtime_tx.send(command);
        if let Some(failure_reason) = failure_reason {
            report_task_failure(task_failure_handler.as_ref(), failure_reason);
        }
    });
}

pub(super) async fn finish_callbacks(
    cancellation_token: &CancellationToken,
    notification_tasks: &mut JoinSet<()>,
    tool_tasks: &mut JoinSet<()>,
    completion: CallbackCompletion,
    task_failure_handler: Option<&TaskFailureHandler>,
) {
    if matches!(completion, CallbackCompletion::Cancel) {
        cancellation_token.cancel();
    }
    match completion {
        CallbackCompletion::DrainNotifications => {
            if tokio::time::timeout(
                NOTIFICATION_DRAIN_TIMEOUT,
                drain_tasks(notification_tasks, "notification", task_failure_handler),
            )
            .await
            .is_err()
            {
                notification_tasks.abort_all();
                report_task_failure(
                    task_failure_handler,
                    "code mode notification drain timed out".to_string(),
                );
                drain_tasks(notification_tasks, "notification", task_failure_handler).await;
            }
        }
        CallbackCompletion::Cancel => {
            drain_tasks(notification_tasks, "notification", task_failure_handler).await;
        }
    }
    cancellation_token.cancel();
    drain_tasks(tool_tasks, "tool", task_failure_handler).await;
}

pub(super) fn report_task_result(
    task_result: Option<Result<(), tokio::task::JoinError>>,
    description: &str,
    task_failure_handler: Option<&TaskFailureHandler>,
) {
    if let Some(Err(err)) = task_result
        && !err.is_cancelled()
    {
        report_task_failure(
            task_failure_handler,
            format!("code mode {description} task failed: {err}"),
        );
    }
}

fn report_task_failure(task_failure_handler: Option<&TaskFailureHandler>, failure_reason: String) {
    warn!("{failure_reason}");
    if let Some(task_failure_handler) = task_failure_handler {
        task_failure_handler(failure_reason);
    }
}

async fn drain_tasks(
    tasks: &mut JoinSet<()>,
    description: &str,
    task_failure_handler: Option<&TaskFailureHandler>,
) {
    while let Some(result) = tasks.join_next().await {
        report_task_result(Some(result), description, task_failure_handler);
    }
}

#[cfg(test)]
#[path = "callbacks_tests.rs"]
mod tests;
