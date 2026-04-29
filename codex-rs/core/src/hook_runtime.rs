use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::agentmemory::context_planner::AgentmemoryContextEndpoint;
use crate::agentmemory::context_planner::AgentmemoryContextEventDetail;
use crate::agentmemory::context_planner::AgentmemoryContextReason;
use crate::agentmemory::context_planner::AgentmemoryContextSkipReason;
use crate::agentmemory::context_planner::AgentmemoryToolCapability;
use crate::agentmemory::context_planner::AutoInjectionRegistration;
use crate::agentmemory::context_planner::PRETOOL_CONTEXT_BUDGET_TOKENS;
use crate::agentmemory::context_planner::QUERY_CONTEXT_BUDGET_TOKENS;
use crate::agentmemory::context_planner::is_trivial_user_turn;
use codex_analytics::HookRunFact;
use codex_analytics::build_track_events_context;
use codex_config::types::MemoryBackend;
use codex_hooks::PermissionRequestDecision;
use codex_hooks::PermissionRequestOutcome;
use codex_hooks::PermissionRequestRequest;
use codex_hooks::PostToolUseOutcome;
use codex_hooks::PostToolUseRequest;
use codex_hooks::PreToolUseOutcome;
use codex_hooks::PreToolUseRequest;
use codex_hooks::SessionStartOutcome;
use codex_hooks::UserPromptSubmitOutcome;
use codex_hooks::UserPromptSubmitRequest;
use codex_otel::HOOK_RUN_DURATION_METRIC;
use codex_otel::HOOK_RUN_METRIC;
use codex_protocol::items::MemoryOperationKind;
use codex_protocol::items::MemoryOperationScope;
use codex_protocol::items::MemoryOperationStatus;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::HookCompletedEvent;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::protocol::HookRunSummary;
use codex_protocol::protocol::HookSource;
use codex_protocol::protocol::HookStartedEvent;
use codex_protocol::protocol::MemoryOperationEvent;
use codex_protocol::protocol::MemoryOperationSource;
use codex_protocol::user_input::UserInput;
use serde_json::Value;
use serde_json::json;

use crate::context::ContextualUserFragment;
use crate::context::HookAdditionalContext;
use crate::event_mapping::parse_turn_item;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tools::hook_names::HookToolName;
use crate::tools::sandboxing::PermissionRequestPayload;

pub(crate) struct HookRuntimeOutcome {
    pub should_stop: bool,
    pub additional_contexts: Vec<String>,
}

pub(crate) struct PreToolUseHookRuntimeOutcome {
    pub block_reason: Option<String>,
    pub additional_contexts: Vec<String>,
}

pub(crate) enum PendingInputHookDisposition {
    Accepted(Box<PendingInputRecord>),
    Blocked { additional_contexts: Vec<String> },
}

pub(crate) enum PendingInputRecord {
    UserMessage {
        content: Vec<UserInput>,
        response_item: ResponseItem,
        additional_contexts: Vec<String>,
    },
    ConversationItem {
        response_item: ResponseItem,
    },
}

struct ContextInjectingHookOutcome {
    hook_events: Vec<HookCompletedEvent>,
    outcome: HookRuntimeOutcome,
}

struct AutomaticContextInjectionArgs {
    reason: AgentmemoryContextReason,
    tool_name: Option<String>,
    tool_capability: Option<AgentmemoryToolCapability>,
    query: Option<String>,
    endpoint: AgentmemoryContextEndpoint,
    fallback_endpoint: Option<AgentmemoryContextEndpoint>,
    request_budget_tokens: Option<usize>,
    context: String,
    retrieval_trace: Option<crate::agentmemory::retrieval_trace::AgentmemoryRetrievalTraceSummary>,
    retrieval_items: Option<Vec<crate::agentmemory::AgentmemoryContextItemSummary>>,
}

impl From<SessionStartOutcome> for ContextInjectingHookOutcome {
    fn from(value: SessionStartOutcome) -> Self {
        let SessionStartOutcome {
            hook_events,
            should_stop,
            stop_reason: _,
            additional_contexts,
        } = value;
        Self {
            hook_events,
            outcome: HookRuntimeOutcome {
                should_stop,
                additional_contexts,
            },
        }
    }
}

impl From<UserPromptSubmitOutcome> for ContextInjectingHookOutcome {
    fn from(value: UserPromptSubmitOutcome) -> Self {
        let UserPromptSubmitOutcome {
            hook_events,
            should_stop,
            stop_reason: _,
            additional_contexts,
        } = value;
        Self {
            hook_events,
            outcome: HookRuntimeOutcome {
                should_stop,
                additional_contexts,
            },
        }
    }
}

async fn emit_automatic_memory_event(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    status: MemoryOperationStatus,
    query: Option<String>,
    summary: String,
    detail: AgentmemoryContextEventDetail,
    context_injected: bool,
) {
    sess.send_event(
        turn_context,
        EventMsg::MemoryOperation(MemoryOperationEvent {
            source: MemoryOperationSource::Automatic,
            operation: MemoryOperationKind::Recall,
            status,
            query,
            summary,
            detail: detail.to_pretty_json(),
            scope: detail.scope,
            context_injected,
        }),
    )
    .await;
}

async fn register_automatic_context_injection(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    args: AutomaticContextInjectionArgs,
) -> Option<String> {
    let AutomaticContextInjectionArgs {
        reason,
        tool_name,
        tool_capability,
        query,
        endpoint,
        fallback_endpoint,
        request_budget_tokens,
        context,
        retrieval_trace,
        retrieval_items,
    } = args;
    let lane_key = reason.lane_key(tool_name.as_deref());
    match sess
        .register_agentmemory_auto_injection(&lane_key, &context)
        .await
    {
        AutoInjectionRegistration::Allowed => {
            emit_automatic_memory_event(
                sess,
                turn_context,
                MemoryOperationStatus::Ready,
                query.clone(),
                automatic_ready_summary(reason, tool_name.as_deref()),
                AgentmemoryContextEventDetail {
                    reason: reason.summary_label(),
                    tool_name,
                    tool_capability,
                    query,
                    endpoint: Some(endpoint),
                    fallback_endpoint,
                    request_budget_tokens,
                    backend_error: None,
                    scope: MemoryOperationScope::Turn,
                    skip_reason: None,
                    duplicate_suppressed: false,
                    fallback_used: fallback_endpoint.is_some(),
                    retrieval_attempted: true,
                    context_injected: true,
                    retrieval_trace,
                    retrieval_items,
                },
                true,
            )
            .await;
            Some(context)
        }
        AutoInjectionRegistration::DuplicateSuppressed => {
            emit_automatic_memory_event(
                sess,
                turn_context,
                MemoryOperationStatus::Skipped,
                query.clone(),
                format!(
                    "Skipped duplicate agentmemory context for {}.",
                    reason.summary_label()
                ),
                AgentmemoryContextEventDetail {
                    reason: reason.summary_label(),
                    tool_name,
                    tool_capability,
                    query,
                    endpoint: Some(endpoint),
                    fallback_endpoint,
                    request_budget_tokens,
                    backend_error: None,
                    scope: MemoryOperationScope::None,
                    skip_reason: Some(AgentmemoryContextSkipReason::DuplicateSuppressed),
                    duplicate_suppressed: true,
                    fallback_used: fallback_endpoint.is_some(),
                    retrieval_attempted: true,
                    context_injected: false,
                    retrieval_trace,
                    retrieval_items,
                },
                false,
            )
            .await;
            None
        }
        AutoInjectionRegistration::MaxAutoInjectionsPerTurn => {
            emit_automatic_memory_event(
                sess,
                turn_context,
                MemoryOperationStatus::Skipped,
                query.clone(),
                "Skipped agentmemory auto-injection because the per-turn injection cap was reached."
                    .to_string(),
                AgentmemoryContextEventDetail {
                    reason: reason.summary_label(),
                    tool_name,
                    tool_capability,
                    query,
                    endpoint: Some(endpoint),
                    fallback_endpoint,
                    request_budget_tokens,
                    backend_error: None,
                    scope: MemoryOperationScope::None,
                    skip_reason: Some(AgentmemoryContextSkipReason::MaxAutoInjectionsPerTurn),
                    duplicate_suppressed: false,
                    fallback_used: fallback_endpoint.is_some(),
                    retrieval_attempted: true,
                    context_injected: false,
                    retrieval_trace,
                    retrieval_items,
                },
                false,
            )
            .await;
            None
        }
    }
}

fn automatic_ready_summary(reason: AgentmemoryContextReason, tool_name: Option<&str>) -> String {
    match (reason, tool_name) {
        (AgentmemoryContextReason::SessionStart, _) => {
            "Auto-injected agentmemory context for session start.".to_string()
        }
        (AgentmemoryContextReason::UserTurn, _) => {
            "Auto-injected agentmemory context for this user turn.".to_string()
        }
        (AgentmemoryContextReason::PreTool, Some(tool_name)) => {
            format!("Auto-injected agentmemory context before tool `{tool_name}`.")
        }
        (AgentmemoryContextReason::PreTool, None) => {
            "Auto-injected agentmemory context before a tool call.".to_string()
        }
    }
}

pub(crate) async fn run_pending_session_start_hooks(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
) -> HookRuntimeOutcome {
    let mut pending_additional_contexts =
        sess.take_pending_session_start_additional_contexts().await;
    let Some(session_start_source) = sess.take_pending_session_start_source().await else {
        if turn_context.config.memories.backend == MemoryBackend::Agentmemory
            && !pending_additional_contexts.is_empty()
        {
            for _ in &pending_additional_contexts {
                emit_automatic_memory_event(
                    sess,
                    turn_context,
                    MemoryOperationStatus::Ready,
                    None,
                    "Auto-injected agentmemory context for session start.".to_string(),
                    AgentmemoryContextEventDetail {
                        reason: AgentmemoryContextReason::SessionStart.summary_label(),
                        tool_name: None,
                        tool_capability: None,
                        query: None,
                        endpoint: Some(AgentmemoryContextEndpoint::SessionStart),
                        fallback_endpoint: None,
                        request_budget_tokens: None,
                        backend_error: None,
                        scope: MemoryOperationScope::Turn,
                        skip_reason: None,
                        duplicate_suppressed: false,
                        fallback_used: false,
                        retrieval_attempted: true,
                        context_injected: true,
                        retrieval_trace: None,
                        retrieval_items: None,
                    },
                    true,
                )
                .await;
            }
        }
        return HookRuntimeOutcome {
            should_stop: false,
            additional_contexts: pending_additional_contexts,
        };
    };

    let request = codex_hooks::SessionStartRequest {
        session_id: sess.conversation_id,
        cwd: turn_context.cwd.clone(),
        transcript_path: sess.hook_transcript_path().await,
        model: turn_context.model_info.slug.clone(),
        permission_mode: hook_permission_mode(turn_context),
        source: session_start_source,
    };

    if turn_context.config.memories.backend == MemoryBackend::Agentmemory {
        let adapter = crate::agentmemory::AgentmemoryAdapter::new();
        let payload = request.clone();
        let memories = turn_context.config.memories.clone();
        tokio::spawn(async move {
            adapter
                .capture_event(
                    "SessionStart",
                    serde_json::to_value(&payload).unwrap_or_default(),
                    &memories,
                )
                .await;
        });
    }

    let preview_runs = sess.hooks().preview_session_start(&request);
    let mut outcome = run_context_injecting_hook(
        sess,
        turn_context,
        preview_runs,
        sess.hooks()
            .run_session_start(request, Some(turn_context.sub_id.clone())),
    )
    .await;
    if turn_context.config.memories.backend == MemoryBackend::Agentmemory
        && !pending_additional_contexts.is_empty()
    {
        for _ in &pending_additional_contexts {
            emit_automatic_memory_event(
                sess,
                turn_context,
                MemoryOperationStatus::Ready,
                None,
                "Auto-injected agentmemory context for session start.".to_string(),
                AgentmemoryContextEventDetail {
                    reason: AgentmemoryContextReason::SessionStart.summary_label(),
                    tool_name: None,
                    tool_capability: None,
                    query: None,
                    endpoint: Some(AgentmemoryContextEndpoint::SessionStart),
                    fallback_endpoint: None,
                    request_budget_tokens: None,
                    backend_error: None,
                    scope: MemoryOperationScope::Turn,
                    skip_reason: None,
                    duplicate_suppressed: false,
                    fallback_used: false,
                    retrieval_attempted: true,
                    context_injected: true,
                    retrieval_trace: None,
                    retrieval_items: None,
                },
                true,
            )
            .await;
        }
    }
    pending_additional_contexts.append(&mut outcome.additional_contexts);
    outcome.additional_contexts = pending_additional_contexts;
    outcome
}

/// Runs matching `PreToolUse` hooks before a tool executes.
///
/// `tool_name` is the canonical name serialized to hook stdin. Matcher aliases
/// are internal compatibility names used only for selecting configured hook
/// handlers.
pub(crate) async fn run_pre_tool_use_hooks(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    tool_use_id: String,
    tool_name: &HookToolName,
    tool_input: &Value,
) -> PreToolUseHookRuntimeOutcome {
    let request = PreToolUseRequest {
        session_id: sess.conversation_id,
        turn_id: turn_context.sub_id.clone(),
        cwd: turn_context.cwd.clone(),
        transcript_path: sess.hook_transcript_path().await,
        model: turn_context.model_info.slug.clone(),
        permission_mode: hook_permission_mode(turn_context),
        tool_name: tool_name.name().to_string(),
        matcher_aliases: tool_name.matcher_aliases().to_vec(),
        tool_use_id,
        tool_input: tool_input.clone(),
    };

    let mut additional_contexts = Vec::new();
    if turn_context.config.memories.backend == MemoryBackend::Agentmemory {
        let adapter = crate::agentmemory::AgentmemoryAdapter::new();
        let observe_adapter = adapter.clone();
        let memories = turn_context.config.memories.clone();
        let observe_memories = memories.clone();
        let payload = json!({
            "session_id": request.session_id.to_string(),
            "turn_id": request.turn_id.clone(),
            "cwd": request.cwd.display().to_string(),
            "model": request.model.clone(),
            "tool_name": request.tool_name.clone(),
            "tool_use_id": request.tool_use_id.clone(),
            "tool_input": request.tool_input.clone(),
        });
        tokio::spawn(async move {
            observe_adapter
                .capture_event("PreToolUse", payload, &observe_memories)
                .await;
        });

        if adapter.inject_context_enabled(&memories)
            && let Some((agentmemory_tool_name, tool_capability, enrichment_input)) =
                agentmemory_tool_context(tool_name, tool_input)
        {
            match adapter
                .file_enrichment_context_result(
                    &sess.conversation_id.to_string(),
                    &enrichment_input,
                    turn_context.cwd.as_path(),
                    &memories,
                )
                .await
            {
                Ok(payload) if !payload.context.trim().is_empty() => {
                    let retrieval_trace = payload.retrieval_trace_summary();
                    let retrieval_items = Some(payload.retrieval_item_summaries());
                    if let Some(context) = register_automatic_context_injection(
                        sess,
                        turn_context,
                        AutomaticContextInjectionArgs {
                            reason: AgentmemoryContextReason::PreTool,
                            tool_name: Some(agentmemory_tool_name),
                            tool_capability: Some(tool_capability),
                            query: None,
                            endpoint: AgentmemoryContextEndpoint::Context,
                            fallback_endpoint: None,
                            request_budget_tokens: Some(PRETOOL_CONTEXT_BUDGET_TOKENS),
                            context: payload.context,
                            retrieval_trace,
                            retrieval_items,
                        },
                    )
                    .await
                    {
                        additional_contexts.push(context);
                    }
                }
                Ok(payload) => {
                    emit_automatic_memory_event(
                            sess,
                            turn_context,
                            MemoryOperationStatus::Empty,
                            None,
                            format!(
                                "Agentmemory enrichment returned no usable context before tool `{agentmemory_tool_name}`."
                            ),
                            AgentmemoryContextEventDetail {
                                reason: AgentmemoryContextReason::PreTool.summary_label(),
                                tool_name: Some(agentmemory_tool_name),
                                tool_capability: Some(tool_capability),
                                query: None,
                                endpoint: Some(AgentmemoryContextEndpoint::Context),
                                fallback_endpoint: None,
                                request_budget_tokens: Some(PRETOOL_CONTEXT_BUDGET_TOKENS),
                                backend_error: None,
                                scope: MemoryOperationScope::None,
                                skip_reason: Some(AgentmemoryContextSkipReason::EmptyResult),
                                duplicate_suppressed: false,
                                fallback_used: false,
                                retrieval_attempted: true,
                                context_injected: false,
                                retrieval_trace: payload.retrieval_trace_summary(),
                                retrieval_items: Some(payload.retrieval_item_summaries()),
                            },
                            false,
                        )
                        .await;
                }
                Err(err) => {
                    tracing::warn!(
                        "failed to enrich agentmemory context before tool {agentmemory_tool_name}: {err}"
                    );
                    emit_automatic_memory_event(
                        sess,
                        turn_context,
                        MemoryOperationStatus::Error,
                        None,
                        format!(
                            "Agentmemory enrichment failed before tool `{agentmemory_tool_name}`."
                        ),
                        AgentmemoryContextEventDetail {
                            reason: AgentmemoryContextReason::PreTool.summary_label(),
                            tool_name: Some(agentmemory_tool_name),
                            tool_capability: Some(tool_capability),
                            query: None,
                            endpoint: Some(AgentmemoryContextEndpoint::Context),
                            fallback_endpoint: None,
                            request_budget_tokens: Some(PRETOOL_CONTEXT_BUDGET_TOKENS),
                            backend_error: Some(err),
                            scope: MemoryOperationScope::None,
                            skip_reason: Some(AgentmemoryContextSkipReason::BackendError),
                            duplicate_suppressed: false,
                            fallback_used: false,
                            retrieval_attempted: true,
                            context_injected: false,
                            retrieval_trace: None,
                            retrieval_items: None,
                        },
                        false,
                    )
                    .await;
                }
            }
        }
    }

    let preview_runs = sess.hooks().preview_pre_tool_use(&request);
    emit_hook_started_events(sess, turn_context, preview_runs).await;

    let PreToolUseOutcome {
        hook_events,
        should_block,
        block_reason,
    } = sess.hooks().run_pre_tool_use(request).await;
    emit_hook_completed_events(sess, turn_context, hook_events).await;

    let block_reason = if should_block {
        block_reason.map(|reason| {
            if (tool_name.name() == "Bash" || tool_name.name() == "apply_patch")
                && let Some(command) = tool_input.get("command").and_then(Value::as_str)
            {
                format!("Command blocked by PreToolUse hook: {reason}. Command: {command}")
            } else {
                format!(
                    "Tool call blocked by PreToolUse hook: {reason}. Tool: {}",
                    tool_name.name()
                )
            }
        })
    } else {
        None
    };

    PreToolUseHookRuntimeOutcome {
        block_reason,
        additional_contexts,
    }
}

fn agentmemory_tool_context(
    tool_name: &HookToolName,
    tool_input: &Value,
) -> Option<(String, AgentmemoryToolCapability, Value)> {
    if tool_name.name() == "apply_patch" {
        let command = tool_input.get("command").and_then(Value::as_str)?;
        let paths = extract_patch_paths(command);
        if paths.is_empty() {
            return None;
        }
        return Some((
            agentmemory_patch_tool_name(command).to_string(),
            AgentmemoryToolCapability::Patch,
            json!({ "paths": paths }),
        ));
    }

    let tool_name = tool_name.name().to_string();
    let capability = AgentmemoryToolCapability::from_tool_name(&tool_name)?;
    Some((tool_name, capability, tool_input.clone()))
}

fn agentmemory_patch_tool_name(command: &str) -> &'static str {
    let mut saw_add = false;
    let mut saw_edit = false;
    for line in command.lines() {
        if line.starts_with("*** Add File: ") {
            saw_add = true;
        } else if line.starts_with("*** Update File: ")
            || line.starts_with("*** Delete File: ")
            || line.starts_with("*** Move to: ")
        {
            saw_edit = true;
        }
    }
    if saw_add && !saw_edit {
        "Write"
    } else {
        "Edit"
    }
}

fn extract_patch_paths(command: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for prefix in [
        "*** Update File: ",
        "*** Add File: ",
        "*** Delete File: ",
        "*** Move to: ",
    ] {
        for line in command.lines() {
            if let Some(path) = line.strip_prefix(prefix)
                && !paths.iter().any(|existing| existing == path)
            {
                paths.push(path.to_string());
            }
        }
    }
    paths
}

// PermissionRequest hooks share the same preview/start/completed event flow as
// other hook types, but they return an optional decision instead of mutating
// tool input or post-run state.
pub(crate) async fn run_permission_request_hooks(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    run_id_suffix: &str,
    payload: PermissionRequestPayload,
) -> Option<PermissionRequestDecision> {
    let request = PermissionRequestRequest {
        session_id: sess.conversation_id,
        turn_id: turn_context.sub_id.clone(),
        cwd: turn_context.cwd.to_path_buf(),
        transcript_path: sess.hook_transcript_path().await,
        model: turn_context.model_info.slug.clone(),
        permission_mode: hook_permission_mode(turn_context),
        tool_name: payload.tool_name.name().to_string(),
        matcher_aliases: payload.tool_name.matcher_aliases().to_vec(),
        run_id_suffix: run_id_suffix.to_string(),
        tool_input: payload.tool_input,
    };
    let preview_runs = sess.hooks().preview_permission_request(&request);
    emit_hook_started_events(sess, turn_context, preview_runs).await;

    let PermissionRequestOutcome {
        hook_events,
        decision,
    } = sess.hooks().run_permission_request(request).await;
    emit_hook_completed_events(sess, turn_context, hook_events).await;

    decision
}

/// Runs matching `PostToolUse` hooks after a tool has produced a successful output.
///
/// The `tool_name`, matcher aliases, `tool_input`, and `tool_response` values are
/// already adapted by the tool handler into the stable hook contract. Passing
/// raw internal tool data here would leak implementation details into user hook
/// matchers and hook logs.
pub(crate) async fn run_post_tool_use_hooks(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    tool_use_id: String,
    tool_name: String,
    matcher_aliases: Vec<String>,
    tool_input: Value,
    tool_response: Value,
) -> PostToolUseOutcome {
    let request = PostToolUseRequest {
        session_id: sess.conversation_id,
        turn_id: turn_context.sub_id.clone(),
        cwd: turn_context.cwd.clone(),
        transcript_path: sess.hook_transcript_path().await,
        model: turn_context.model_info.slug.clone(),
        permission_mode: hook_permission_mode(turn_context),
        tool_name,
        matcher_aliases,
        tool_use_id,
        tool_input,
        tool_response,
    };
    if turn_context.config.memories.backend == MemoryBackend::Agentmemory {
        let adapter = crate::agentmemory::AgentmemoryAdapter::new();
        let payload = serde_json::to_value(&request).unwrap_or_default();
        let memories = turn_context.config.memories.clone();
        tokio::spawn(async move {
            adapter
                .capture_event("PostToolUse", payload, &memories)
                .await;
        });
    }

    let preview_runs = sess.hooks().preview_post_tool_use(&request);
    emit_hook_started_events(sess, turn_context, preview_runs).await;

    let outcome = sess.hooks().run_post_tool_use(request).await;
    emit_hook_completed_events(sess, turn_context, outcome.hook_events.clone()).await;
    outcome
}

pub(crate) async fn run_user_prompt_submit_hooks(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    prompt: String,
) -> HookRuntimeOutcome {
    let request = UserPromptSubmitRequest {
        session_id: sess.conversation_id,
        turn_id: turn_context.sub_id.clone(),
        cwd: turn_context.cwd.clone(),
        transcript_path: sess.hook_transcript_path().await,
        model: turn_context.model_info.slug.clone(),
        permission_mode: hook_permission_mode(turn_context),
        prompt,
    };
    let mut additional_contexts = Vec::new();
    if turn_context.config.memories.backend == MemoryBackend::Agentmemory {
        let adapter = crate::agentmemory::AgentmemoryAdapter::new();
        let observe_adapter = adapter.clone();
        let payload = serde_json::to_value(&request).unwrap_or_default();
        let memories = turn_context.config.memories.clone();
        tokio::spawn(async move {
            observe_adapter
                .capture_event("UserPromptSubmit", payload, &memories)
                .await;
        });

        sess.begin_agentmemory_turn().await;
        let trimmed_prompt = request.prompt.trim();
        let query = (!trimmed_prompt.is_empty()).then(|| trimmed_prompt.to_string());
        if is_trivial_user_turn(trimmed_prompt) {
            emit_automatic_memory_event(
                sess,
                turn_context,
                MemoryOperationStatus::Skipped,
                query,
                "Skipped automatic agentmemory retrieval for a trivial user turn.".to_string(),
                AgentmemoryContextEventDetail {
                    reason: AgentmemoryContextReason::UserTurn.summary_label(),
                    tool_name: None,
                    tool_capability: None,
                    query: None,
                    endpoint: None,
                    fallback_endpoint: None,
                    request_budget_tokens: None,
                    backend_error: None,
                    scope: MemoryOperationScope::None,
                    skip_reason: Some(AgentmemoryContextSkipReason::TrivialUserTurn),
                    duplicate_suppressed: false,
                    fallback_used: false,
                    retrieval_attempted: false,
                    context_injected: false,
                    retrieval_trace: None,
                    retrieval_items: None,
                },
                false,
            )
            .await;
        } else if let Some(query) = query {
            match adapter
                .refresh_context_result(
                    &sess.conversation_id.to_string(),
                    turn_context.cwd.as_path(),
                    &query,
                    &turn_context.config.memories,
                )
                .await
            {
                Ok(payload) if !payload.context.trim().is_empty() => {
                    let retrieval_trace = payload.retrieval_trace_summary();
                    let retrieval_items = Some(payload.retrieval_item_summaries());
                    if let Some(context) = register_automatic_context_injection(
                        sess,
                        turn_context,
                        AutomaticContextInjectionArgs {
                            reason: AgentmemoryContextReason::UserTurn,
                            tool_name: None,
                            tool_capability: None,
                            query: Some(query),
                            endpoint: AgentmemoryContextEndpoint::Context,
                            fallback_endpoint: None,
                            request_budget_tokens: Some(QUERY_CONTEXT_BUDGET_TOKENS),
                            context: payload.context,
                            retrieval_trace,
                            retrieval_items,
                        },
                    )
                    .await
                    {
                        additional_contexts.push(context);
                    }
                }
                Ok(payload) => {
                    emit_automatic_memory_event(
                        sess,
                        turn_context,
                        MemoryOperationStatus::Empty,
                        Some(query.clone()),
                        "Agentmemory retrieval returned no usable context for this user turn."
                            .to_string(),
                        AgentmemoryContextEventDetail {
                            reason: AgentmemoryContextReason::UserTurn.summary_label(),
                            tool_name: None,
                            tool_capability: None,
                            query: Some(query),
                            endpoint: Some(AgentmemoryContextEndpoint::Context),
                            fallback_endpoint: None,
                            request_budget_tokens: Some(QUERY_CONTEXT_BUDGET_TOKENS),
                            backend_error: None,
                            scope: MemoryOperationScope::None,
                            skip_reason: Some(AgentmemoryContextSkipReason::EmptyResult),
                            duplicate_suppressed: false,
                            fallback_used: false,
                            retrieval_attempted: true,
                            context_injected: false,
                            retrieval_trace: payload.retrieval_trace_summary(),
                            retrieval_items: Some(payload.retrieval_item_summaries()),
                        },
                        false,
                    )
                    .await;
                }
                Err(err) => {
                    tracing::warn!(
                        "failed to retrieve agentmemory context on prompt submit: {err}"
                    );
                    emit_automatic_memory_event(
                        sess,
                        turn_context,
                        MemoryOperationStatus::Error,
                        Some(query.clone()),
                        "Agentmemory retrieval failed for this user turn.".to_string(),
                        AgentmemoryContextEventDetail {
                            reason: AgentmemoryContextReason::UserTurn.summary_label(),
                            tool_name: None,
                            tool_capability: None,
                            query: Some(query),
                            endpoint: Some(AgentmemoryContextEndpoint::Context),
                            fallback_endpoint: None,
                            request_budget_tokens: Some(QUERY_CONTEXT_BUDGET_TOKENS),
                            backend_error: Some(err),
                            scope: MemoryOperationScope::None,
                            skip_reason: Some(AgentmemoryContextSkipReason::BackendError),
                            duplicate_suppressed: false,
                            fallback_used: false,
                            retrieval_attempted: true,
                            context_injected: false,
                            retrieval_trace: None,
                            retrieval_items: None,
                        },
                        false,
                    )
                    .await;
                }
            }
        }
    }

    let preview_runs = sess.hooks().preview_user_prompt_submit(&request);
    let mut outcome = run_context_injecting_hook(
        sess,
        turn_context,
        preview_runs,
        sess.hooks().run_user_prompt_submit(request),
    )
    .await;
    additional_contexts.append(&mut outcome.additional_contexts);
    outcome.additional_contexts = additional_contexts;
    outcome
}

pub(crate) async fn inspect_pending_input(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    pending_input_item: ResponseInputItem,
) -> PendingInputHookDisposition {
    let response_item = ResponseItem::from(pending_input_item);
    if let Some(TurnItem::UserMessage(user_message)) = parse_turn_item(&response_item) {
        let user_prompt_submit_outcome =
            run_user_prompt_submit_hooks(sess, turn_context, user_message.message()).await;
        if user_prompt_submit_outcome.should_stop {
            PendingInputHookDisposition::Blocked {
                additional_contexts: user_prompt_submit_outcome.additional_contexts,
            }
        } else {
            PendingInputHookDisposition::Accepted(Box::new(PendingInputRecord::UserMessage {
                content: user_message.content,
                response_item,
                additional_contexts: user_prompt_submit_outcome.additional_contexts,
            }))
        }
    } else {
        PendingInputHookDisposition::Accepted(Box::new(PendingInputRecord::ConversationItem {
            response_item,
        }))
    }
}

pub(crate) async fn record_pending_input(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    pending_input: PendingInputRecord,
) {
    match pending_input {
        PendingInputRecord::UserMessage {
            content,
            response_item,
            additional_contexts,
        } => {
            sess.record_user_prompt_and_emit_turn_item(
                turn_context.as_ref(),
                content.as_slice(),
                response_item,
            )
            .await;
            record_additional_contexts(sess, turn_context, additional_contexts).await;
        }
        PendingInputRecord::ConversationItem { response_item } => {
            sess.record_conversation_items(turn_context, std::slice::from_ref(&response_item))
                .await;
        }
    }
}

async fn run_context_injecting_hook<Fut, Outcome>(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    preview_runs: Vec<HookRunSummary>,
    outcome_future: Fut,
) -> HookRuntimeOutcome
where
    Fut: Future<Output = Outcome>,
    Outcome: Into<ContextInjectingHookOutcome>,
{
    emit_hook_started_events(sess, turn_context, preview_runs).await;

    let outcome = outcome_future.await.into();
    emit_hook_completed_events(sess, turn_context, outcome.hook_events).await;
    outcome.outcome
}

pub(crate) async fn record_additional_contexts(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    additional_contexts: Vec<String>,
) {
    let developer_messages = additional_context_messages(additional_contexts);
    if developer_messages.is_empty() {
        return;
    }

    sess.record_conversation_items(turn_context, developer_messages.as_slice())
        .await;
}

fn additional_context_messages(additional_contexts: Vec<String>) -> Vec<ResponseItem> {
    additional_contexts
        .into_iter()
        .map(HookAdditionalContext::new)
        .map(ContextualUserFragment::into)
        .collect()
}

async fn emit_hook_started_events(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    preview_runs: Vec<HookRunSummary>,
) {
    for run in preview_runs {
        sess.send_event(
            turn_context,
            EventMsg::HookStarted(HookStartedEvent {
                turn_id: Some(turn_context.sub_id.clone()),
                run,
            }),
        )
        .await;
    }
}

pub(crate) async fn emit_hook_completed_events(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    completed_events: Vec<HookCompletedEvent>,
) {
    for completed in completed_events {
        emit_hook_completed_metrics(turn_context, &completed);
        track_hook_completed_analytics(sess, turn_context, &completed);
        sess.send_event(turn_context, EventMsg::HookCompleted(completed))
            .await;
    }
}

fn emit_hook_completed_metrics(turn_context: &TurnContext, completed: &HookCompletedEvent) {
    let tags = hook_run_metric_tags(&completed.run);
    turn_context
        .session_telemetry
        .counter(HOOK_RUN_METRIC, /*inc*/ 1, &tags);
    if let Some(duration_ms) = completed.run.duration_ms
        && let Ok(duration_ms) = u64::try_from(duration_ms)
    {
        turn_context.session_telemetry.record_duration(
            HOOK_RUN_DURATION_METRIC,
            Duration::from_millis(duration_ms),
            &tags,
        );
    }
}

fn track_hook_completed_analytics(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    completed: &HookCompletedEvent,
) {
    let (tracking, hook) =
        hook_run_analytics_payload(sess.conversation_id.to_string(), turn_context, completed);
    sess.services
        .analytics_events_client
        .track_hook_run(tracking, hook);
}

fn hook_run_analytics_payload(
    thread_id: String,
    turn_context: &TurnContext,
    completed: &HookCompletedEvent,
) -> (codex_analytics::TrackEventsContext, HookRunFact) {
    (
        build_track_events_context(
            turn_context.model_info.slug.clone(),
            thread_id,
            completed
                .turn_id
                .clone()
                .unwrap_or_else(|| turn_context.sub_id.clone()),
        ),
        HookRunFact {
            event_name: completed.run.event_name,
            hook_source: completed.run.source,
            status: completed.run.status,
        },
    )
}

fn hook_run_metric_tags(run: &HookRunSummary) -> [(&'static str, &'static str); 3] {
    let hook_name = match run.event_name {
        HookEventName::PreToolUse => "PreToolUse",
        HookEventName::PermissionRequest => "PermissionRequest",
        HookEventName::PostToolUse => "PostToolUse",
        HookEventName::PostToolUseFailure => "PostToolUseFailure",
        HookEventName::PreCompact => "PreCompact",
        HookEventName::SessionStart => "SessionStart",
        HookEventName::SubagentStart => "SubagentStart",
        HookEventName::SubagentStop => "SubagentStop",
        HookEventName::Notification => "Notification",
        HookEventName::TaskCompleted => "TaskCompleted",
        HookEventName::UserPromptSubmit => "UserPromptSubmit",
        HookEventName::Stop => "Stop",
        HookEventName::SessionEnd => "SessionEnd",
    };
    let hook_source = match run.source {
        HookSource::System => "system",
        HookSource::User => "user",
        HookSource::Project => "project",
        HookSource::Mdm => "mdm",
        HookSource::SessionFlags => "session_flags",
        HookSource::Plugin => "plugin",
        HookSource::LegacyManagedConfigFile => "legacy_managed_config_file",
        HookSource::LegacyManagedConfigMdm => "legacy_managed_config_mdm",
        HookSource::Unknown => "unknown",
    };
    let status = match run.status {
        HookRunStatus::Running => "running",
        HookRunStatus::Completed => "completed",
        HookRunStatus::Failed => "failed",
        HookRunStatus::Blocked => "blocked",
        HookRunStatus::Stopped => "stopped",
    };

    [
        ("hook_name", hook_name),
        ("source", hook_source),
        ("status", status),
    ]
}

fn hook_permission_mode(turn_context: &TurnContext) -> String {
    match turn_context.approval_policy.value() {
        AskForApproval::Never => "bypassPermissions",
        AskForApproval::UnlessTrusted
        | AskForApproval::OnFailure
        | AskForApproval::OnRequest
        | AskForApproval::Granular(_) => "default",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use codex_protocol::models::ContentItem;
    use codex_protocol::protocol::HookEventName;
    use codex_protocol::protocol::HookExecutionMode;
    use codex_protocol::protocol::HookHandlerType;
    use codex_protocol::protocol::HookRunStatus;
    use codex_protocol::protocol::HookScope;
    use codex_protocol::protocol::HookSource;
    use pretty_assertions::assert_eq;

    use super::additional_context_messages;
    use super::hook_run_analytics_payload;
    use super::hook_run_metric_tags;
    use crate::session::tests::make_session_and_context;
    use codex_protocol::protocol::HookCompletedEvent;
    use codex_protocol::protocol::HookRunSummary;
    use codex_utils_absolute_path::test_support::PathBufExt;
    use codex_utils_absolute_path::test_support::test_path_buf;

    #[test]
    fn additional_context_messages_stay_separate_and_ordered() {
        let messages = additional_context_messages(vec![
            "first tide note".to_string(),
            "second tide note".to_string(),
        ]);

        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages
                .iter()
                .map(|message| match message {
                    codex_protocol::models::ResponseItem::Message { role, content, .. } => {
                        let text = content
                            .iter()
                            .map(|item| match item {
                                ContentItem::InputText { text } => text.as_str(),
                                ContentItem::InputImage { .. } | ContentItem::OutputText { .. } => {
                                    panic!("expected input text content, got {item:?}")
                                }
                            })
                            .collect::<String>();
                        (role.as_str(), text)
                    }
                    other => panic!("expected developer message, got {other:?}"),
                })
                .collect::<Vec<_>>(),
            vec![
                ("developer", "first tide note".to_string()),
                ("developer", "second tide note".to_string()),
            ],
        );
    }

    #[tokio::test]
    async fn hook_run_analytics_payload_uses_completed_turn_id() {
        let (_session, turn_context) = make_session_and_context().await;
        let completed = HookCompletedEvent {
            turn_id: Some("turn-from-hook".to_string()),
            run: sample_hook_run(HookRunStatus::Blocked, HookSource::Project),
        };

        let (tracking, hook) =
            hook_run_analytics_payload("thread-123".to_string(), &turn_context, &completed);

        assert_eq!(tracking.thread_id, "thread-123");
        assert_eq!(tracking.turn_id, "turn-from-hook");
        assert_eq!(tracking.model_slug, turn_context.model_info.slug);
        assert_eq!(hook.event_name, HookEventName::Stop);
        assert_eq!(hook.hook_source, HookSource::Project);
        assert_eq!(hook.status, HookRunStatus::Blocked);
    }

    #[tokio::test]
    async fn hook_run_analytics_payload_falls_back_to_turn_context_id() {
        let (_session, turn_context) = make_session_and_context().await;
        let completed = HookCompletedEvent {
            turn_id: None,
            run: sample_hook_run(HookRunStatus::Failed, HookSource::Unknown),
        };

        let (tracking, hook) =
            hook_run_analytics_payload("thread-123".to_string(), &turn_context, &completed);

        assert_eq!(tracking.turn_id, turn_context.sub_id);
        assert_eq!(hook.hook_source, HookSource::Unknown);
        assert_eq!(hook.status, HookRunStatus::Failed);
    }

    #[test]
    fn hook_run_metric_tags_match_analytics_shape() {
        let run = sample_hook_run(HookRunStatus::Blocked, HookSource::Project);

        assert_eq!(
            hook_run_metric_tags(&run),
            [
                ("hook_name", "Stop"),
                ("source", "project"),
                ("status", "blocked"),
            ]
        );
    }

    #[test]
    fn hook_run_metric_tags_include_expanded_hook_sources() {
        let run = sample_hook_run(HookRunStatus::Completed, HookSource::LegacyManagedConfigMdm);

        assert_eq!(
            hook_run_metric_tags(&run),
            [
                ("hook_name", "Stop"),
                ("source", "legacy_managed_config_mdm"),
                ("status", "completed"),
            ]
        );
    }

    fn sample_hook_run(status: HookRunStatus, source: HookSource) -> HookRunSummary {
        HookRunSummary {
            id: "stop:0:/tmp/hooks.json".to_string(),
            event_name: HookEventName::Stop,
            handler_type: HookHandlerType::Command,
            execution_mode: HookExecutionMode::Sync,
            scope: HookScope::Turn,
            source_path: test_path_buf("/tmp/hooks.json").abs(),
            source,
            display_order: 0,
            status,
            status_message: None,
            started_at: 10,
            completed_at: Some(37),
            duration_ms: Some(27),
            entries: Vec::new(),
        }
    }
}
