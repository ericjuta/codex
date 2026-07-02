use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use codex_code_mode::CellId;
use codex_code_mode::CodeModeNestedToolCall;
use codex_code_mode::CodeModeSessionDelegate;
use codex_code_mode::NotificationFuture;
use codex_code_mode::ToolInvocationFuture;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::function_call_output_content_items_to_text;
use serde_json::Value as JsonValue;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::ExecContext;
use super::call_nested_tool;
use super::truncate_code_mode_result;
use crate::session::step_context::StepContext;
use crate::tools::ToolRouter;
use crate::tools::context::SharedTurnDiffTracker;
use crate::tools::parallel::ToolCallRuntime;

pub(super) struct CodeModeDispatchBroker {
    dispatch_tx: async_channel::Sender<DispatchMessage>,
    dispatch_rx: async_channel::Receiver<DispatchMessage>,
    dispatch_gates: Arc<Mutex<HashMap<CellId, watch::Sender<bool>>>>,
    active_turn_id: Arc<Mutex<Option<String>>>,
}

impl CodeModeDispatchBroker {
    pub(super) fn new() -> Self {
        let (dispatch_tx, dispatch_rx) = async_channel::unbounded();
        Self {
            dispatch_tx,
            dispatch_rx,
            dispatch_gates: Arc::new(Mutex::new(HashMap::new())),
            active_turn_id: Arc::new(Mutex::new(None)),
        }
    }

    pub(super) fn mark_cell_ready_for_dispatch(&self, cell_id: &CellId) {
        dispatch_gate(&self.dispatch_gates, cell_id).send_replace(true);
    }

    pub(super) fn close_cell(&self, cell_id: &CellId) {
        remove_dispatch_gate(&self.dispatch_gates, cell_id);
    }

    pub(super) fn start_turn_worker(
        &self,
        exec: ExecContext,
        router: Arc<ToolRouter>,
        step_context: Arc<StepContext>,
        tracker: SharedTurnDiffTracker,
        turn_id: String,
        turn_end_cleanup: Box<dyn FnOnce() + Send>,
    ) -> CodeModeDispatchWorker {
        set_active_turn_id(&self.active_turn_id, Some(turn_id.clone()));
        let tool_runtime =
            ToolCallRuntime::new(router, Arc::clone(&exec.session), step_context, tracker);
        let host = Arc::new(CoreTurnHost { exec, tool_runtime });
        let dispatch_rx = self.dispatch_rx.clone();
        let dispatch_gates = Arc::clone(&self.dispatch_gates);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let worker_turn_id = turn_id.clone();
        tokio::spawn(async move {
            loop {
                let message = tokio::select! {
                    _ = &mut shutdown_rx => break,
                    message = dispatch_rx.recv() => message.ok(),
                };
                let Some(message) = message else {
                    break;
                };
                match message {
                    DispatchMessage::Notify {
                        submitting_turn_id,
                        call_id,
                        cell_id,
                        text,
                        max_output_tokens,
                        cancellation_token,
                        response_tx,
                    } => {
                        if !message_belongs_to_turn_or_ready_cell(
                            submitting_turn_id.as_deref(),
                            &worker_turn_id,
                            &dispatch_gates,
                            &cell_id,
                        ) {
                            let _ = response_tx.send(Err(stale_turn_message(submitting_turn_id)));
                            continue;
                        }
                        let response = if wait_until_cell_ready_for_dispatch(
                            &dispatch_gates,
                            &cell_id,
                            &cancellation_token,
                        )
                        .await
                        {
                            host.notify(call_id, cell_id, text, max_output_tokens).await
                        } else {
                            remove_dispatch_gate(&dispatch_gates, &cell_id);
                            Err("code mode notification cancelled".to_string())
                        };
                        let _ = response_tx.send(response);
                    }
                    DispatchMessage::InvokeTool {
                        submitting_turn_id,
                        invocation,
                        cancellation_token,
                        response_tx,
                    } => {
                        let cell_id = invocation.cell_id.clone();
                        if !message_belongs_to_turn_or_ready_cell(
                            submitting_turn_id.as_deref(),
                            &worker_turn_id,
                            &dispatch_gates,
                            &cell_id,
                        ) {
                            let _ = response_tx.send(Err(stale_turn_message(submitting_turn_id)));
                            continue;
                        }
                        if !wait_until_cell_ready_for_dispatch(
                            &dispatch_gates,
                            &cell_id,
                            &cancellation_token,
                        )
                        .await
                        {
                            remove_dispatch_gate(&dispatch_gates, &cell_id);
                            let _ = response_tx
                                .send(Err("code mode nested tool call cancelled".to_string()));
                            continue;
                        }
                        let host = Arc::clone(&host);
                        tokio::spawn(async move {
                            let response = tokio::select! {
                                response = host.invoke_tool(
                                    invocation,
                                    cancellation_token.clone(),
                                ) => response,
                                _ = cancellation_token.cancelled() => return,
                            };
                            let _ = response_tx.send(response);
                        });
                    }
                }
            }
        });
        CodeModeDispatchWorker {
            shutdown_tx: Some(shutdown_tx),
            active_turn_id: Arc::clone(&self.active_turn_id),
            turn_id,
            turn_end_cleanup: Some(turn_end_cleanup),
        }
    }

    fn current_turn_id(&self) -> Option<String> {
        self.active_turn_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

fn set_active_turn_id(active_turn_id: &Mutex<Option<String>>, turn_id: Option<String>) {
    *active_turn_id
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = turn_id;
}

fn clear_active_turn_id(active_turn_id: &Mutex<Option<String>>, turn_id: &str) {
    let mut active_turn_id = active_turn_id
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if active_turn_id.as_deref() == Some(turn_id) {
        *active_turn_id = None;
    }
}

fn message_belongs_to_turn_or_ready_cell(
    submitting_turn_id: Option<&str>,
    active_turn_id: &str,
    dispatch_gates: &Mutex<HashMap<CellId, watch::Sender<bool>>>,
    cell_id: &CellId,
) -> bool {
    submitting_turn_id == Some(active_turn_id) || cell_ready_for_dispatch(dispatch_gates, cell_id)
}

fn stale_turn_message(submitting_turn_id: Option<String>) -> String {
    match submitting_turn_id {
        Some(turn_id) => {
            format!("code mode nested tool call was submitted by inactive turn {turn_id}")
        }
        None => "code mode nested tool call was submitted without an active turn".to_string(),
    }
}

fn dispatch_gate(
    dispatch_gates: &Mutex<HashMap<CellId, watch::Sender<bool>>>,
    cell_id: &CellId,
) -> watch::Sender<bool> {
    let mut dispatch_gates = match dispatch_gates.lock() {
        Ok(dispatch_gates) => dispatch_gates,
        Err(poisoned) => poisoned.into_inner(),
    };
    dispatch_gates
        .entry(cell_id.clone())
        .or_insert_with(|| watch::channel(false).0)
        .clone()
}

fn remove_dispatch_gate(
    dispatch_gates: &Mutex<HashMap<CellId, watch::Sender<bool>>>,
    cell_id: &CellId,
) {
    let mut dispatch_gates = match dispatch_gates.lock() {
        Ok(dispatch_gates) => dispatch_gates,
        Err(poisoned) => poisoned.into_inner(),
    };
    dispatch_gates.remove(cell_id);
}

fn cell_ready_for_dispatch(
    dispatch_gates: &Mutex<HashMap<CellId, watch::Sender<bool>>>,
    cell_id: &CellId,
) -> bool {
    let dispatch_gates = match dispatch_gates.lock() {
        Ok(dispatch_gates) => dispatch_gates,
        Err(poisoned) => poisoned.into_inner(),
    };
    dispatch_gates
        .get(cell_id)
        .is_some_and(|ready_tx| *ready_tx.borrow())
}

async fn wait_until_cell_ready_for_dispatch(
    dispatch_gates: &Mutex<HashMap<CellId, watch::Sender<bool>>>,
    cell_id: &CellId,
    cancellation_token: &CancellationToken,
) -> bool {
    if cancellation_token.is_cancelled() {
        return false;
    }
    let mut ready_rx = dispatch_gate(dispatch_gates, cell_id).subscribe();
    loop {
        if *ready_rx.borrow_and_update() {
            return true;
        }
        tokio::select! {
            changed = ready_rx.changed() => {
                if changed.is_err() {
                    return false;
                }
            }
            _ = cancellation_token.cancelled() => return false,
        }
    }
}

impl CodeModeSessionDelegate for CodeModeDispatchBroker {
    fn invoke_tool<'a>(
        &'a self,
        invocation: CodeModeNestedToolCall,
        cancellation_token: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async move {
            if cancellation_token.is_cancelled() {
                return Err("code mode nested tool call cancelled".to_string());
            }
            let (response_tx, response_rx) = oneshot::channel();
            self.dispatch_tx
                .send(DispatchMessage::InvokeTool {
                    submitting_turn_id: self.current_turn_id(),
                    invocation,
                    cancellation_token: cancellation_token.clone(),
                    response_tx,
                })
                .await
                .map_err(|_| "code mode nested tool dispatcher is unavailable".to_string())?;
            tokio::select! {
                response = response_rx => response
                    .map_err(|_| "code mode nested tool dispatcher stopped".to_string())?,
                _ = cancellation_token.cancelled() => {
                    Err("code mode nested tool call cancelled".to_string())
                }
            }
        })
    }

    fn notify<'a>(
        &'a self,
        call_id: String,
        cell_id: CellId,
        text: String,
        max_output_tokens: Option<usize>,
        cancellation_token: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async move {
            if cancellation_token.is_cancelled() {
                return Err("code mode notification cancelled".to_string());
            }
            let (response_tx, response_rx) = oneshot::channel();
            self.dispatch_tx
                .send(DispatchMessage::Notify {
                    submitting_turn_id: self.current_turn_id(),
                    call_id,
                    cell_id,
                    text,
                    max_output_tokens,
                    cancellation_token: cancellation_token.clone(),
                    response_tx,
                })
                .await
                .map_err(|_| "code mode notification dispatcher is unavailable".to_string())?;
            tokio::select! {
                response = response_rx => response
                    .map_err(|_| "code mode notification dispatcher stopped".to_string())?,
                _ = cancellation_token.cancelled() => {
                    Err("code mode notification cancelled".to_string())
                }
            }
        })
    }

    fn cell_closed(&self, cell_id: &CellId) {
        self.close_cell(cell_id);
    }
}

enum DispatchMessage {
    InvokeTool {
        submitting_turn_id: Option<String>,
        invocation: CodeModeNestedToolCall,
        cancellation_token: CancellationToken,
        response_tx: oneshot::Sender<Result<JsonValue, String>>,
    },
    Notify {
        submitting_turn_id: Option<String>,
        call_id: String,
        cell_id: CellId,
        text: String,
        max_output_tokens: Option<usize>,
        cancellation_token: CancellationToken,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
}

pub(crate) struct CodeModeDispatchWorker {
    shutdown_tx: Option<oneshot::Sender<()>>,
    active_turn_id: Arc<Mutex<Option<String>>>,
    turn_id: String,
    turn_end_cleanup: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for CodeModeDispatchWorker {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        clear_active_turn_id(&self.active_turn_id, &self.turn_id);
        if let Some(turn_end_cleanup) = self.turn_end_cleanup.take() {
            turn_end_cleanup();
        }
    }
}

struct CoreTurnHost {
    exec: ExecContext,
    tool_runtime: ToolCallRuntime,
}

impl CoreTurnHost {
    async fn invoke_tool(
        &self,
        invocation: CodeModeNestedToolCall,
        cancellation_token: CancellationToken,
    ) -> Result<JsonValue, String> {
        call_nested_tool(
            self.exec.clone(),
            self.tool_runtime.clone(),
            invocation,
            cancellation_token,
        )
        .await
        .map_err(|error| error.to_string())
    }

    async fn notify(
        &self,
        call_id: String,
        cell_id: CellId,
        text: String,
        max_output_tokens: Option<usize>,
    ) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        // Deliver the notification as a developer message rather than a tool
        // output: the exec call already receives its own output through the
        // tool runtime, and the Responses API rejects a second output bound to
        // the same call_id.
        let truncated = truncate_code_mode_result(
            vec![FunctionCallOutputContentItem::InputText { text }],
            max_output_tokens,
        );
        let Some(text) = function_call_output_content_items_to_text(&truncated) else {
            return Ok(());
        };
        let message = format!(
            "<code_mode_notification cell_id=\"{cell_id}\" call_id=\"{call_id}\">\n{text}\n</code_mode_notification>"
        );
        self.exec
            .session
            .inject_if_running(vec![ResponseItem::Message {
                id: None,
                role: "developer".to_string(),
                content: vec![ContentItem::InputText { text: message }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }])
            .await
            .map_err(|_| {
                format!("failed to inject exec notify message for cell {cell_id}: no active turn")
            })
    }
}
