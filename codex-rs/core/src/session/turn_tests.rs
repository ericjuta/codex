use super::*;
use crate::tools::registry::AnyToolResponse;
use crate::tools::registry::ToolRegistry;
use crate::turn_diff_tracker::TurnDiffTracker;
use codex_extension_api::ExtensionData;
use codex_extension_api::TurnItemContributor;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::openai_models::ToolMode;
use pretty_assertions::assert_eq;
use std::sync::Arc;

struct RewriteAgentMessageContributor;

impl TurnItemContributor for RewriteAgentMessageContributor {
    fn contribute<'a>(
        &'a self,
        _thread_store: &'a ExtensionData,
        _turn_store: &'a ExtensionData,
        item: &'a mut TurnItem,
    ) -> codex_extension_api::ExtensionFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let TurnItem::AgentMessage(agent_message) = item {
                agent_message.content = vec![AgentMessageContent::Text {
                    text: "plan contributed assistant text".to_string(),
                }];
            }
            Ok(())
        })
    }
}

#[derive(Default)]
struct RecordingCodeModeSession {
    terminated_cells: std::sync::Mutex<Vec<codex_code_mode::CellId>>,
}

impl RecordingCodeModeSession {
    fn terminated_cells(&self) -> Vec<codex_code_mode::CellId> {
        self.terminated_cells
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

struct RecordingCodeModeSessionProvider {
    session: Arc<RecordingCodeModeSession>,
}

impl codex_code_mode::CodeModeSessionProvider for RecordingCodeModeSessionProvider {
    fn create_session<'a>(
        &'a self,
        _delegate: Arc<dyn codex_code_mode::CodeModeSessionDelegate>,
    ) -> codex_code_mode::CodeModeSessionProviderFuture<'a> {
        let session: Arc<dyn codex_code_mode::CodeModeSession> = self.session.clone();
        Box::pin(async move { Ok(session) })
    }
}

impl codex_code_mode::CodeModeSession for RecordingCodeModeSession {
    fn execute<'a>(
        &'a self,
        request: codex_code_mode::ExecuteRequest,
    ) -> codex_code_mode::CodeModeSessionResultFuture<'a, codex_code_mode::StartedCell> {
        Box::pin(async move {
            let (_tx, rx) = tokio::sync::oneshot::channel();
            Ok(codex_code_mode::StartedCell::new(
                codex_code_mode::CellId::new(format!("started-{}", request.tool_call_id)),
                rx,
            ))
        })
    }

    fn wait<'a>(
        &'a self,
        request: codex_code_mode::WaitRequest,
    ) -> codex_code_mode::CodeModeSessionResultFuture<'a, codex_code_mode::WaitOutcome> {
        Box::pin(async move {
            Ok(codex_code_mode::WaitOutcome::MissingCell(
                codex_code_mode::RuntimeResponse::Terminated {
                    cell_id: request.cell_id,
                    content_items: Vec::new(),
                },
            ))
        })
    }

    fn terminate<'a>(
        &'a self,
        cell_id: codex_code_mode::CellId,
    ) -> codex_code_mode::CodeModeSessionResultFuture<'a, codex_code_mode::WaitOutcome> {
        Box::pin(async move {
            self.terminated_cells
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(cell_id.clone());
            Ok(codex_code_mode::WaitOutcome::MissingCell(
                codex_code_mode::RuntimeResponse::Terminated {
                    cell_id,
                    content_items: Vec::new(),
                },
            ))
        })
    }

    fn shutdown<'a>(&'a self) -> codex_code_mode::CodeModeSessionResultFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

fn assistant_output_text(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some("msg-1".to_string()),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

async fn session_with_registered_code_mode_cell(
    cell_id: &codex_code_mode::CellId,
) -> (
    Arc<Session>,
    Arc<TurnContext>,
    Arc<RecordingCodeModeSession>,
) {
    let (mut session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    turn_context.model_info.tool_mode = Some(ToolMode::CodeMode);
    let recording_session = Arc::new(RecordingCodeModeSession::default());
    session.services.code_mode_service =
        crate::tools::code_mode::CodeModeService::new(Arc::new(RecordingCodeModeSessionProvider {
            session: Arc::clone(&recording_session),
        }));
    session
        .services
        .code_mode_service
        .execute(codex_code_mode::ExecuteRequest {
            tool_call_id: "init-call".to_string(),
            enabled_tools: Vec::new(),
            source: String::new(),
            yield_time_ms: None,
            max_output_tokens: None,
        })
        .await
        .expect("recording code-mode session should initialize");
    session
        .services
        .code_mode_service
        .register_started_cell(turn_context.sub_id.as_str(), cell_id);

    (Arc::new(session), Arc::new(turn_context), recording_session)
}

fn start_code_mode_worker(
    session: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
) -> impl Drop + use<> {
    session
        .services
        .code_mode_service
        .start_turn_worker(
            session,
            StepContext::for_test(Arc::clone(turn_context)),
            Arc::new(ToolRouter::from_parts(
                ToolRegistry::empty_for_test(),
                Vec::new(),
            )),
            Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new())),
        )
        .expect("code-mode turn worker should start")
}

async fn drain_single_tool_response(
    session: Arc<Session>,
    turn_context: Arc<TurnContext>,
    disclosed_code_mode_cell_id: Option<String>,
) -> CodexResult<()> {
    let response_input = ResponseInputItem::CustomToolCallOutput {
        call_id: "call-1".to_string(),
        name: None,
        output: FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text("Script running".to_string()),
            success: Some(true),
        },
    };
    let mut in_flight: FuturesOrdered<BoxFuture<'static, CodexResult<AnyToolResponse>>> =
        FuturesOrdered::new();
    in_flight.push_back(Box::pin(async move {
        Ok(AnyToolResponse::new(
            response_input,
            disclosed_code_mode_cell_id,
        ))
    }));
    let mut wait_for_cancelled_drain = VecDeque::from([true]);

    drain_in_flight(
        &mut in_flight,
        &mut wait_for_cancelled_drain,
        session,
        turn_context,
        &CancellationToken::new(),
    )
    .await
}

#[tokio::test]
async fn drain_in_flight_marks_disclosed_code_cell_before_turn_cleanup() -> CodexResult<()> {
    let cell_id = codex_code_mode::CellId::new("cell-1".to_string());
    let (session, turn_context, recording_session) =
        session_with_registered_code_mode_cell(&cell_id).await;
    let worker = start_code_mode_worker(&session, &turn_context);

    drain_single_tool_response(
        Arc::clone(&session),
        Arc::clone(&turn_context),
        Some(cell_id.to_string()),
    )
    .await?;
    drop(worker);
    tokio::task::yield_now().await;

    assert_eq!(recording_session.terminated_cells(), Vec::new());
    Ok(())
}

#[tokio::test]
async fn turn_cleanup_terminates_undisclosed_code_cell_after_drain() -> CodexResult<()> {
    let cell_id = codex_code_mode::CellId::new("cell-1".to_string());
    let (session, turn_context, recording_session) =
        session_with_registered_code_mode_cell(&cell_id).await;
    let worker = start_code_mode_worker(&session, &turn_context);

    drain_single_tool_response(Arc::clone(&session), Arc::clone(&turn_context), None).await?;
    drop(worker);
    for _ in 0..20 {
        if recording_session.terminated_cells() == vec![cell_id.clone()] {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(recording_session.terminated_cells(), vec![cell_id]);
    Ok(())
}

#[tokio::test]
async fn plan_mode_uses_contributed_turn_item_for_last_agent_message() {
    let (mut session, turn_context) = crate::session::tests::make_session_and_context().await;
    let mut builder = codex_extension_api::ExtensionRegistryBuilder::new();
    builder.turn_item_contributor(Arc::new(RewriteAgentMessageContributor));
    session.services.extensions = Arc::new(builder.build());
    let turn_store = ExtensionData::new(turn_context.sub_id.clone());
    let mut state = PlanModeStreamState::new(&turn_context.sub_id);
    let mut last_agent_message = None;
    let item = assistant_output_text("original assistant text");

    let handled = handle_assistant_item_done_in_plan_mode(
        &session,
        &turn_context,
        &turn_store,
        &item,
        &mut state,
        /*previously_active_item*/ None,
        &mut last_agent_message,
    )
    .await;

    assert!(handled);
    assert_eq!(
        last_agent_message.as_deref(),
        Some("plan contributed assistant text")
    );
}
