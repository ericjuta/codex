use crate::agentmemory::AgentmemoryAdapter;
use crate::config::types::MemoryBackend;
use crate::function_tool::FunctionCallError;
use crate::session::agentmemory_ops::MemoryOperationEventArgs;
use crate::session::agentmemory_ops::send_memory_operation_event_with_scope;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use codex_protocol::items::MemoryOperationKind;
use codex_protocol::items::MemoryOperationScope;
use codex_protocol::items::MemoryOperationStatus;
use codex_protocol::models::DeveloperInstructions;
use codex_protocol::protocol::MemoryOperationSource;
use serde::Deserialize;
use serde_json::Value as JsonValue;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MemoryRecallScopeArg {
    Turn,
    Thread,
}

impl From<MemoryRecallScopeArg> for MemoryOperationScope {
    fn from(value: MemoryRecallScopeArg) -> Self {
        match value {
            MemoryRecallScopeArg::Turn => Self::Turn,
            MemoryRecallScopeArg::Thread => Self::Thread,
        }
    }
}

#[derive(Debug, Deserialize)]
struct MemoryRecallArgs {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    scope: Option<MemoryRecallScopeArg>,
}

#[derive(Debug, Deserialize)]
struct MemoryRememberArgs {
    content: String,
}

#[derive(Debug, Deserialize)]
struct MemoryQueryArgs {
    #[serde(default)]
    query: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryActionsArgs {
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryMissionsArgs {
    #[serde(default)]
    mission_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct MemoryHandoffsArgs {
    #[serde(default)]
    handoff_packet_id: Option<String>,
    #[serde(default)]
    scope_type: Option<String>,
    #[serde(default)]
    scope_id: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct MemoryHandoffGenerateArgs {
    #[serde(default)]
    scope_type: Option<String>,
    #[serde(default)]
    scope_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MemoryFrontierArgs {
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct MemoryBranchOverlaysArgs {
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct MemoryDossiersArgs {
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    refresh: Option<bool>,
}

pub struct MemoryRecallHandler;
pub struct MemoryRememberHandler;
pub struct MemoryLessonsHandler;
pub struct MemoryCrystalsHandler;
pub struct MemoryInsightsHandler;
pub struct MemoryActionsHandler;
pub struct MemoryMissionsHandler;
pub struct MemoryBranchOverlaysHandler;
pub struct MemoryGuardrailsHandler;
pub struct MemoryDecisionsHandler;
pub struct MemoryDossiersHandler;
pub struct MemoryRoutineCandidatesHandler;
pub struct MemoryHandoffsHandler;
pub struct MemoryHandoffGenerateHandler;
pub struct MemoryFrontierHandler;
pub struct MemoryNextHandler;

async fn emit_event(
    session: &Session,
    turn: &TurnContext,
    operation: MemoryOperationKind,
    status: MemoryOperationStatus,
    query: Option<String>,
    summary: String,
    detail: Option<String>,
) {
    emit_event_with_scope(
        session,
        turn,
        MemoryOperationEventArgs {
            source: MemoryOperationSource::Assistant,
            operation,
            status,
            query,
            summary,
            detail,
            context_injected: false,
        },
        MemoryOperationScope::None,
    )
    .await;
}

async fn emit_event_with_scope(
    session: &Session,
    turn: &TurnContext,
    args: MemoryOperationEventArgs,
    scope: MemoryOperationScope,
) {
    send_memory_operation_event_with_scope(session, &turn.sub_id, args, scope).await;
}

fn require_agentmemory_backend(
    turn: &TurnContext,
    tool_name: &str,
) -> Result<(), FunctionCallError> {
    if turn.config.memories.backend != MemoryBackend::Agentmemory {
        return Err(FunctionCallError::RespondToModel(format!(
            "{tool_name} requires agentmemory backend"
        )));
    }
    Ok(())
}

fn json_text_output(response: JsonValue) -> Result<FunctionToolOutput, FunctionCallError> {
    let content = serde_json::to_string(&response).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize memory tool response: {err}"))
    })?;
    let mut output = FunctionToolOutput::from_text(content, Some(true));
    output.post_tool_use_response = Some(response);
    Ok(output)
}

fn json_success(response: &JsonValue) -> bool {
    response
        .get("success")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
}

fn json_error_detail(response: &JsonValue) -> Option<String> {
    response
        .get("error")
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| serde_json::to_string_pretty(response).ok())
}

fn json_count(response: &JsonValue, key: &str) -> usize {
    response
        .get(key)
        .and_then(JsonValue::as_array)
        .map_or(0, Vec::len)
}

fn is_valid_handoff_scope_type(scope_type: &str) -> bool {
    matches!(scope_type, "action" | "mission" | "session")
}

async fn handle_recall(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
    require_agentmemory_backend(turn.as_ref(), "memory_recall")?;

    let args: MemoryRecallArgs = parse_arguments(&arguments)?;
    let query = args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty());
    let scope = args.scope.unwrap_or(MemoryRecallScopeArg::Turn);
    let applied_scope = MemoryOperationScope::from(scope);

    let adapter = AgentmemoryAdapter::new();
    let response = match adapter
        .recall_for_runtime(
            &session.conversation_id.to_string(),
            turn.cwd.as_path(),
            query,
            &turn.config.memories,
        )
        .await
    {
        Ok(response) => response,
        Err(err) => {
            emit_event_with_scope(
                &session,
                turn.as_ref(),
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Assistant,
                    operation: MemoryOperationKind::Recall,
                    status: MemoryOperationStatus::Error,
                    query: args.query.clone(),
                    summary: "Assistant memory recall failed.".to_string(),
                    detail: Some(err.clone()),
                    context_injected: false,
                },
                applied_scope,
            )
            .await;
            return Err(FunctionCallError::RespondToModel(format!(
                "memory_recall failed: {err}"
            )));
        }
    };

    if response.recalled && applied_scope == MemoryOperationScope::Thread {
        let message: codex_protocol::models::ResponseItem = DeveloperInstructions::new(format!(
            "<agentmemory-recall>\n{}\n</agentmemory-recall>",
            response.context
        ))
        .into();
        session
            .record_conversation_items(turn.as_ref(), std::slice::from_ref(&message))
            .await;
    }

    emit_event_with_scope(
        &session,
        turn.as_ref(),
        MemoryOperationEventArgs {
            source: MemoryOperationSource::Assistant,
            operation: MemoryOperationKind::Recall,
            status: if response.recalled {
                MemoryOperationStatus::Ready
            } else {
                MemoryOperationStatus::Empty
            },
            query: args.query.clone(),
            summary: if response.recalled && applied_scope == MemoryOperationScope::Thread {
                "Assistant recalled memory context and persisted it to the thread.".to_string()
            } else if response.recalled {
                "Assistant recalled memory context for this turn.".to_string()
            } else {
                "Assistant found no relevant memory context for this turn.".to_string()
            },
            detail: response.recalled.then_some(response.context.clone()),
            context_injected: response.recalled && applied_scope == MemoryOperationScope::Thread,
        },
        applied_scope,
    )
    .await;

    let response = serde_json::json!({
        "recalled": response.recalled,
        "context": response.context,
        "scope": match applied_scope {
            MemoryOperationScope::Turn => "turn",
            MemoryOperationScope::Thread => "thread",
            MemoryOperationScope::None => "none",
        },
    });
    let response = serde_json::to_value(response).map_err(|err| {
        FunctionCallError::Fatal(format!("failed to encode memory_recall response: {err}"))
    })?;
    json_text_output(response)
}

async fn handle_remember(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_remember handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_remember")?;

    let args: MemoryRememberArgs = parse_arguments(&arguments)?;
    let content = args.content.trim();
    if content.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "memory_remember requires non-empty content".to_string(),
        ));
    }

    let adapter = AgentmemoryAdapter::new();
    match adapter
        .remember_memory(content, &turn.config.memories)
        .await
    {
        Ok(response) if json_success(&response) => {
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Remember,
                MemoryOperationStatus::Ready,
                None,
                "Assistant saved durable memory.".to_string(),
                serde_json::to_string_pretty(&response).ok(),
            )
            .await;
            json_text_output(response)
        }
        Ok(response) => {
            let detail = json_error_detail(&response);
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Remember,
                MemoryOperationStatus::Error,
                None,
                "Assistant memory remember failed.".to_string(),
                detail.clone(),
            )
            .await;
            Err(FunctionCallError::RespondToModel(
                detail.unwrap_or_else(|| "memory_remember failed".to_string()),
            ))
        }
        Err(err) => {
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Remember,
                MemoryOperationStatus::Error,
                None,
                "Assistant memory remember failed.".to_string(),
                Some(err.clone()),
            )
            .await;
            Err(FunctionCallError::RespondToModel(format!(
                "memory_remember failed: {err}"
            )))
        }
    }
}

async fn handle_lessons(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_lessons handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_lessons")?;

    let args: MemoryQueryArgs = parse_arguments(&arguments)?;
    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    let response = match args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(query) => {
            adapter
                .search_lessons(query, project.as_path(), &turn.config.memories)
                .await
        }
        None => {
            adapter
                .list_lessons(project.as_path(), &turn.config.memories)
                .await
        }
    };

    handle_review_response(
        session.as_ref(),
        turn.as_ref(),
        MemoryOperationKind::Lessons,
        args.query,
        response,
        "memory_lessons",
        "lessons",
    )
    .await
}

async fn handle_crystals(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_crystals handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_crystals")?;
    let _: MemoryQueryArgs = parse_arguments(&arguments)?;

    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    let response = adapter
        .list_crystals(project.as_path(), &turn.config.memories)
        .await;
    handle_review_response(
        session.as_ref(),
        turn.as_ref(),
        MemoryOperationKind::Crystals,
        None,
        response,
        "memory_crystals",
        "crystals",
    )
    .await
}

async fn handle_insights(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_insights handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_insights")?;

    let args: MemoryQueryArgs = parse_arguments(&arguments)?;
    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    let response = match args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(query) => {
            adapter
                .search_insights(query, project.as_path(), &turn.config.memories)
                .await
        }
        None => {
            adapter
                .list_insights(project.as_path(), &turn.config.memories)
                .await
        }
    };

    handle_review_response(
        session.as_ref(),
        turn.as_ref(),
        MemoryOperationKind::Insights,
        args.query,
        response,
        "memory_insights",
        "insights",
    )
    .await
}

async fn handle_actions(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_actions handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_actions")?;

    let args: MemoryActionsArgs = parse_arguments(&arguments)?;
    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    let response = adapter
        .list_actions(
            project.as_path(),
            args.status.as_deref(),
            None,
            None,
            &turn.config.memories,
        )
        .await;

    handle_review_response(
        session.as_ref(),
        turn.as_ref(),
        MemoryOperationKind::Actions,
        args.status,
        response,
        "memory_actions",
        "actions",
    )
    .await
}

async fn handle_missions(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_missions handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_missions")?;

    let args: MemoryMissionsArgs = parse_arguments(&arguments)?;
    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    let query = args
        .mission_id
        .clone()
        .or(args.status.clone())
        .or(args.owner.clone());
    let response = match args
        .mission_id
        .as_deref()
        .map(str::trim)
        .filter(|mission_id| !mission_id.is_empty())
    {
        Some(mission_id) => adapter.get_mission(mission_id, &turn.config.memories).await,
        None => {
            adapter
                .list_missions(
                    project.as_path(),
                    args.status.as_deref(),
                    args.owner.as_deref(),
                    args.limit,
                    &turn.config.memories,
                )
                .await
        }
    };

    match response {
        Ok(response) if json_success(&response) => {
            let count = json_count(&response, "missions");
            let status = if args.mission_id.is_some() || count > 0 {
                MemoryOperationStatus::Ready
            } else {
                MemoryOperationStatus::Empty
            };
            let summary = if let Some(mission_id) = args.mission_id.as_deref() {
                format!("Assistant reviewed mission `{mission_id}`.")
            } else if count > 0 {
                format!("Assistant reviewed {count} missions.")
            } else if let Some(status_filter) = args.status.as_deref() {
                format!("Assistant found no `{status_filter}` missions.")
            } else {
                "Assistant found no missions.".to_string()
            };
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Missions,
                status,
                query,
                summary,
                serde_json::to_string_pretty(&response).ok(),
            )
            .await;
            json_text_output(response)
        }
        Ok(response) => {
            let detail = json_error_detail(&response);
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Missions,
                MemoryOperationStatus::Error,
                query,
                "Assistant mission review failed.".to_string(),
                detail.clone(),
            )
            .await;
            Err(FunctionCallError::RespondToModel(
                detail.unwrap_or_else(|| "memory_missions failed".to_string()),
            ))
        }
        Err(err) => {
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Missions,
                MemoryOperationStatus::Error,
                query,
                "Assistant mission review failed.".to_string(),
                Some(err.clone()),
            )
            .await;
            Err(FunctionCallError::RespondToModel(format!(
                "memory_missions failed: {err}"
            )))
        }
    }
}

async fn handle_branch_overlays(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_branch_overlays handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_branch_overlays")?;

    let args: MemoryBranchOverlaysArgs = parse_arguments(&arguments)?;
    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    let response = adapter
        .list_branch_overlays(
            project.as_path(),
            args.branch.as_deref(),
            args.limit,
            &turn.config.memories,
        )
        .await;

    handle_review_response(
        session.as_ref(),
        turn.as_ref(),
        MemoryOperationKind::BranchOverlays,
        args.branch,
        response,
        "memory_branch_overlays",
        "overlays",
    )
    .await
}

async fn handle_guardrails(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_guardrails handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_guardrails")?;

    let args: MemoryQueryArgs = parse_arguments(&arguments)?;
    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    let response = match args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        Some(query) => {
            adapter
                .search_guardrails(query, project.as_path(), None, &turn.config.memories)
                .await
        }
        None => {
            adapter
                .list_guardrails(project.as_path(), None, &turn.config.memories)
                .await
        }
    };

    handle_review_response(
        session.as_ref(),
        turn.as_ref(),
        MemoryOperationKind::Guardrails,
        args.query,
        response,
        "memory_guardrails",
        "guardrails",
    )
    .await
}

async fn handle_decisions(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_decisions handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_decisions")?;

    let args: MemoryQueryArgs = parse_arguments(&arguments)?;
    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    let response = match args
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        Some(query) => {
            adapter
                .search_decisions(query, project.as_path(), None, &turn.config.memories)
                .await
        }
        None => {
            adapter
                .list_decisions(project.as_path(), None, &turn.config.memories)
                .await
        }
    };

    handle_review_response(
        session.as_ref(),
        turn.as_ref(),
        MemoryOperationKind::Decisions,
        args.query,
        response,
        "memory_decisions",
        "decisions",
    )
    .await
}

async fn handle_dossiers(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_dossiers handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_dossiers")?;

    let args: MemoryDossiersArgs = parse_arguments(&arguments)?;
    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    let response = match args
        .file_path
        .as_deref()
        .map(str::trim)
        .filter(|file_path| !file_path.is_empty())
    {
        Some(file_path) => {
            adapter
                .get_dossier(
                    project.as_path(),
                    file_path,
                    None,
                    args.refresh.unwrap_or(false),
                    &turn.config.memories,
                )
                .await
        }
        None => {
            adapter
                .list_dossiers(project.as_path(), None, &turn.config.memories)
                .await
        }
    };

    match response {
        Ok(response) if json_success(&response) => {
            let has_dossier = response.get("dossier").is_some();
            let count = json_count(&response, "dossiers");
            let status = if has_dossier || count > 0 {
                MemoryOperationStatus::Ready
            } else {
                MemoryOperationStatus::Empty
            };
            let summary = if let Some(file_path) = args.file_path.as_deref() {
                format!("Reviewed dossier for `{file_path}`.")
            } else if count > 0 {
                format!("Assistant reviewed {count} dossiers.")
            } else {
                "Assistant found no dossiers.".to_string()
            };
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Dossiers,
                status,
                args.file_path.clone(),
                summary,
                serde_json::to_string_pretty(&response).ok(),
            )
            .await;
            json_text_output(response)
        }
        Ok(response) => {
            let detail = json_error_detail(&response);
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Dossiers,
                MemoryOperationStatus::Error,
                args.file_path.clone(),
                "Assistant dossier review failed.".to_string(),
                detail.clone(),
            )
            .await;
            Err(FunctionCallError::RespondToModel(
                detail.unwrap_or_else(|| "memory_dossiers failed".to_string()),
            ))
        }
        Err(err) => {
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Dossiers,
                MemoryOperationStatus::Error,
                args.file_path,
                "Assistant dossier review failed.".to_string(),
                Some(err.clone()),
            )
            .await;
            Err(FunctionCallError::RespondToModel(format!(
                "memory_dossiers failed: {err}"
            )))
        }
    }
}

async fn handle_routine_candidates(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_routine_candidates handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_routine_candidates")?;
    let _: MemoryQueryArgs = parse_arguments(&arguments)?;

    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    let response = adapter
        .list_routine_candidates(project.as_path(), None, &turn.config.memories)
        .await;

    handle_review_response(
        session.as_ref(),
        turn.as_ref(),
        MemoryOperationKind::RoutineCandidates,
        None,
        response,
        "memory_routine_candidates",
        "routineCandidates",
    )
    .await
}

async fn handle_handoffs(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_handoffs handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_handoffs")?;

    let args: MemoryHandoffsArgs = parse_arguments(&arguments)?;
    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    let query = args
        .handoff_packet_id
        .clone()
        .or(args.scope_id.clone())
        .or(args.scope_type.clone());
    let response = match args
        .handoff_packet_id
        .as_deref()
        .map(str::trim)
        .filter(|handoff_packet_id| !handoff_packet_id.is_empty())
    {
        Some(handoff_packet_id) => {
            adapter
                .get_handoff(handoff_packet_id, &turn.config.memories)
                .await
        }
        None => {
            adapter
                .list_handoffs(
                    project.as_path(),
                    args.scope_type.as_deref(),
                    args.scope_id.as_deref(),
                    args.limit,
                    &turn.config.memories,
                )
                .await
        }
    };

    match response {
        Ok(response) if json_success(&response) => {
            let count = json_count(&response, "handoffPackets");
            let status = if args.handoff_packet_id.is_some() || count > 0 {
                MemoryOperationStatus::Ready
            } else {
                MemoryOperationStatus::Empty
            };
            let summary = if let Some(handoff_packet_id) = args.handoff_packet_id.as_deref() {
                format!("Assistant reviewed handoff packet `{handoff_packet_id}`.")
            } else if count > 0 {
                format!("Assistant reviewed {count} handoff packets.")
            } else {
                "Assistant found no handoff packets.".to_string()
            };
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Handoffs,
                status,
                query,
                summary,
                serde_json::to_string_pretty(&response).ok(),
            )
            .await;
            json_text_output(response)
        }
        Ok(response) => {
            let detail = json_error_detail(&response);
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Handoffs,
                MemoryOperationStatus::Error,
                query,
                "Assistant handoff review failed.".to_string(),
                detail.clone(),
            )
            .await;
            Err(FunctionCallError::RespondToModel(
                detail.unwrap_or_else(|| "memory_handoffs failed".to_string()),
            ))
        }
        Err(err) => {
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Handoffs,
                MemoryOperationStatus::Error,
                query,
                "Assistant handoff review failed.".to_string(),
                Some(err.clone()),
            )
            .await;
            Err(FunctionCallError::RespondToModel(format!(
                "memory_handoffs failed: {err}"
            )))
        }
    }
}

async fn handle_handoff_generate(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_handoff_generate handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_handoff_generate")?;

    let args: MemoryHandoffGenerateArgs = parse_arguments(&arguments)?;
    let resolved_scope_type = args.scope_type.unwrap_or_else(|| "session".to_string());
    if !is_valid_handoff_scope_type(resolved_scope_type.as_str()) {
        return Err(FunctionCallError::RespondToModel(
            "memory_handoff_generate scope_type must be one of: action, mission, session"
                .to_string(),
        ));
    }

    let resolved_scope_id = match (resolved_scope_type.as_str(), args.scope_id) {
        ("session", Some(scope_id)) => scope_id,
        ("session", None) => session.conversation_id.to_string(),
        ("mission" | "action", Some(scope_id)) => scope_id,
        ("mission" | "action", None) => {
            return Err(FunctionCallError::RespondToModel(
                "memory_handoff_generate requires scope_id for mission and action scopes"
                    .to_string(),
            ));
        }
        _ => unreachable!(),
    };

    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    match adapter
        .generate_handoff(
            resolved_scope_type.as_str(),
            resolved_scope_id.as_str(),
            project.as_path(),
            &turn.config.memories,
        )
        .await
    {
        Ok(response) if json_success(&response) => {
            let summary = response
                .pointer("/handoffPacket/id")
                .and_then(JsonValue::as_str)
                .map(|handoff_id| format!("Assistant generated handoff packet `{handoff_id}`."))
                .unwrap_or_else(|| "Assistant generated a handoff packet.".to_string());
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::HandoffGenerate,
                MemoryOperationStatus::Ready,
                Some(format!("{resolved_scope_type}:{resolved_scope_id}")),
                summary,
                serde_json::to_string_pretty(&response).ok(),
            )
            .await;
            json_text_output(response)
        }
        Ok(response) => {
            let detail = json_error_detail(&response);
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::HandoffGenerate,
                MemoryOperationStatus::Error,
                Some(format!("{resolved_scope_type}:{resolved_scope_id}")),
                "Assistant handoff generation failed.".to_string(),
                detail.clone(),
            )
            .await;
            Err(FunctionCallError::RespondToModel(detail.unwrap_or_else(
                || "memory_handoff_generate failed".to_string(),
            )))
        }
        Err(err) => {
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::HandoffGenerate,
                MemoryOperationStatus::Error,
                Some(format!("{resolved_scope_type}:{resolved_scope_id}")),
                "Assistant handoff generation failed.".to_string(),
                Some(err.clone()),
            )
            .await;
            Err(FunctionCallError::RespondToModel(format!(
                "memory_handoff_generate failed: {err}"
            )))
        }
    }
}

async fn handle_frontier(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_frontier handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_frontier")?;

    let args: MemoryFrontierArgs = parse_arguments(&arguments)?;
    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    let agent_id = session.conversation_id.to_string();
    match adapter
        .frontier(
            project.as_path(),
            Some(agent_id.as_str()),
            args.limit,
            &turn.config.memories,
        )
        .await
    {
        Ok(response) if json_success(&response) => {
            let count = json_count(&response, "frontier");
            let status = if count > 0 {
                MemoryOperationStatus::Ready
            } else {
                MemoryOperationStatus::Empty
            };
            let summary = response
                .get("totalUnblocked")
                .and_then(JsonValue::as_u64)
                .map(|total_unblocked| {
                    if count > 0 {
                        format!(
                            "Assistant reviewed {count} frontier suggestions ({total_unblocked} unblocked total)."
                        )
                    } else {
                        "Assistant found no unblocked frontier suggestions.".to_string()
                    }
                })
                .unwrap_or_else(|| {
                    if count > 0 {
                        format!("Assistant reviewed {count} frontier suggestions.")
                    } else {
                        "Assistant found no unblocked frontier suggestions.".to_string()
                    }
                });
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Frontier,
                status,
                None,
                summary,
                serde_json::to_string_pretty(&response).ok(),
            )
            .await;
            json_text_output(response)
        }
        Ok(response) => {
            let detail = json_error_detail(&response);
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Frontier,
                MemoryOperationStatus::Error,
                None,
                "Assistant frontier review failed.".to_string(),
                detail.clone(),
            )
            .await;
            Err(FunctionCallError::RespondToModel(
                detail.unwrap_or_else(|| "memory_frontier failed".to_string()),
            ))
        }
        Err(err) => {
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Frontier,
                MemoryOperationStatus::Error,
                None,
                "Assistant frontier review failed.".to_string(),
                Some(err.clone()),
            )
            .await;
            Err(FunctionCallError::RespondToModel(format!(
                "memory_frontier failed: {err}"
            )))
        }
    }
}

async fn handle_next(invocation: ToolInvocation) -> Result<FunctionToolOutput, FunctionCallError> {
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
                "memory_next handler received unsupported payload".to_string(),
            ));
        }
    };
    require_agentmemory_backend(turn.as_ref(), "memory_next")?;
    let _: serde_json::Map<String, JsonValue> = parse_arguments(&arguments)?;

    let adapter = AgentmemoryAdapter::new();
    let project = crate::agentmemory::workspace_project(turn.cwd.as_path());
    let agent_id = session.conversation_id.to_string();
    match adapter
        .next_action(
            project.as_path(),
            Some(agent_id.as_str()),
            &turn.config.memories,
        )
        .await
    {
        Ok(response) if json_success(&response) => {
            let status = if response.get("suggestion").is_some() {
                MemoryOperationStatus::Ready
            } else {
                MemoryOperationStatus::Empty
            };
            let summary = response
                .get("message")
                .and_then(JsonValue::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| "Assistant reviewed the next suggested action.".to_string());
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Next,
                status,
                None,
                summary,
                serde_json::to_string_pretty(&response).ok(),
            )
            .await;
            json_text_output(response)
        }
        Ok(response) => {
            let detail = json_error_detail(&response);
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Next,
                MemoryOperationStatus::Error,
                None,
                "Assistant next-action review failed.".to_string(),
                detail.clone(),
            )
            .await;
            Err(FunctionCallError::RespondToModel(
                detail.unwrap_or_else(|| "memory_next failed".to_string()),
            ))
        }
        Err(err) => {
            emit_event(
                &session,
                turn.as_ref(),
                MemoryOperationKind::Next,
                MemoryOperationStatus::Error,
                None,
                "Assistant next-action review failed.".to_string(),
                Some(err.clone()),
            )
            .await;
            Err(FunctionCallError::RespondToModel(format!(
                "memory_next failed: {err}"
            )))
        }
    }
}

async fn handle_review_response(
    session: &Session,
    turn: &TurnContext,
    operation: MemoryOperationKind,
    query: Option<String>,
    response: Result<JsonValue, String>,
    tool_name: &str,
    count_key: &str,
) -> Result<FunctionToolOutput, FunctionCallError> {
    match response {
        Ok(response) if json_success(&response) => {
            let count = json_count(&response, count_key);
            let status = if count > 0 {
                MemoryOperationStatus::Ready
            } else {
                MemoryOperationStatus::Empty
            };
            let label = tool_name.trim_start_matches("memory_");
            let summary = if count > 0 {
                format!("Assistant reviewed {count} {label}.")
            } else {
                format!("Assistant found no {label}.")
            };
            emit_event(
                session,
                turn,
                operation,
                status,
                query,
                summary,
                serde_json::to_string_pretty(&response).ok(),
            )
            .await;
            json_text_output(response)
        }
        Ok(response) => {
            let detail = json_error_detail(&response);
            emit_event(
                session,
                turn,
                operation,
                MemoryOperationStatus::Error,
                query,
                format!("Assistant {tool_name} review failed."),
                detail.clone(),
            )
            .await;
            Err(FunctionCallError::RespondToModel(
                detail.unwrap_or_else(|| format!("{tool_name} failed")),
            ))
        }
        Err(err) => {
            emit_event(
                session,
                turn,
                operation,
                MemoryOperationStatus::Error,
                query,
                format!("Assistant {tool_name} review failed."),
                Some(err.clone()),
            )
            .await;
            Err(FunctionCallError::RespondToModel(format!(
                "{tool_name} failed: {err}"
            )))
        }
    }
}

impl ToolHandler for MemoryRecallHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_recall(invocation).await
    }
}

impl ToolHandler for MemoryRememberHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_remember(invocation).await
    }
}

impl ToolHandler for MemoryLessonsHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_lessons(invocation).await
    }
}

impl ToolHandler for MemoryCrystalsHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_crystals(invocation).await
    }
}

impl ToolHandler for MemoryInsightsHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_insights(invocation).await
    }
}

impl ToolHandler for MemoryActionsHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_actions(invocation).await
    }
}

impl ToolHandler for MemoryMissionsHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_missions(invocation).await
    }
}

impl ToolHandler for MemoryBranchOverlaysHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_branch_overlays(invocation).await
    }
}

impl ToolHandler for MemoryGuardrailsHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_guardrails(invocation).await
    }
}

impl ToolHandler for MemoryDecisionsHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_decisions(invocation).await
    }
}

impl ToolHandler for MemoryDossiersHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_dossiers(invocation).await
    }
}

impl ToolHandler for MemoryRoutineCandidatesHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_routine_candidates(invocation).await
    }
}

impl ToolHandler for MemoryHandoffsHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_handoffs(invocation).await
    }
}

impl ToolHandler for MemoryHandoffGenerateHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_handoff_generate(invocation).await
    }
}

impl ToolHandler for MemoryFrontierHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_frontier(invocation).await
    }
}

impl ToolHandler for MemoryNextHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        handle_next(invocation).await
    }
}
