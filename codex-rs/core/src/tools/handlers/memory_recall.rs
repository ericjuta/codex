use async_trait::async_trait;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::MemoryOperationEvent;
use codex_protocol::protocol::MemoryOperationSource;
use serde::Deserialize;

use crate::config::types::MemoryBackend;
use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use codex_protocol::items::MemoryOperationKind;
use codex_protocol::items::MemoryOperationStatus;

#[derive(Debug, Deserialize)]
struct MemoryRecallArgs {
    #[serde(default)]
    query: Option<String>,
}

pub struct MemoryRecallHandler;

#[async_trait]
impl ToolHandler for MemoryRecallHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "memory_recall handler received unsupported payload".to_string(),
                ));
            }
        };

        if turn.config.memories.backend != MemoryBackend::Agentmemory {
            return Err(FunctionCallError::RespondToModel(
                "memory_recall requires agentmemory backend".to_string(),
            ));
        }

        let args: MemoryRecallArgs = parse_arguments(&arguments)?;
        let query = args
            .query
            .as_deref()
            .map(str::trim)
            .filter(|query| !query.is_empty());

        let adapter = crate::agentmemory::AgentmemoryAdapter::new();
        let response = match adapter
            .recall_for_runtime(
                &session.conversation_id.to_string(),
                turn.cwd.as_path(),
                query,
            )
            .await
        {
            Ok(response) => response,
            Err(err) => {
                session
                    .send_event(
                        turn.as_ref(),
                        EventMsg::MemoryOperation(MemoryOperationEvent {
                            source: MemoryOperationSource::Assistant,
                            operation: MemoryOperationKind::Recall,
                            status: MemoryOperationStatus::Error,
                            query: args.query.clone(),
                            summary: "Assistant memory recall failed.".to_string(),
                            detail: Some(err.to_string()),
                            context_injected: false,
                        }),
                    )
                    .await;
                return Err(FunctionCallError::RespondToModel(format!(
                    "memory_recall failed: {err}"
                )));
            }
        };

        session
            .send_event(
                turn.as_ref(),
                EventMsg::MemoryOperation(if response.recalled {
                    MemoryOperationEvent {
                        source: MemoryOperationSource::Assistant,
                        operation: MemoryOperationKind::Recall,
                        status: MemoryOperationStatus::Ready,
                        query: args.query.clone(),
                        summary: "Assistant recalled memory context for this turn.".to_string(),
                        detail: Some(response.context.clone()),
                        context_injected: false,
                    }
                } else {
                    MemoryOperationEvent {
                        source: MemoryOperationSource::Assistant,
                        operation: MemoryOperationKind::Recall,
                        status: MemoryOperationStatus::Empty,
                        query: args.query.clone(),
                        summary: "Assistant found no relevant memory context for this turn."
                            .to_string(),
                        detail: None,
                        context_injected: false,
                    }
                }),
            )
            .await;

        let content = serde_json::to_string(&response).map_err(|err| {
            FunctionCallError::Fatal(format!("failed to serialize memory_recall response: {err}"))
        })?;

        let mut output = FunctionToolOutput::from_text(content, Some(true));
        output.post_tool_use_response = Some(serde_json::to_value(&response).map_err(|err| {
            FunctionCallError::Fatal(format!(
                "failed to encode memory_recall post-tool response: {err}"
            ))
        })?);
        Ok(output)
    }
}
