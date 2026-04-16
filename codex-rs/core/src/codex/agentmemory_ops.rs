use super::Event;
use super::Session;
use crate::agentmemory::AgentmemoryAdapter;
use crate::agentmemory::workspace_project;
use crate::config::Config;
use codex_protocol::items::MemoryOperationKind;
use codex_protocol::items::MemoryOperationStatus;
use codex_protocol::models::DeveloperInstructions;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::MemoryOperationEvent;
use codex_protocol::protocol::MemoryOperationSource;
use serde_json::Value as JsonValue;
use std::path::PathBuf;
use std::sync::Arc;

struct MemoryOperationEventArgs {
    source: MemoryOperationSource,
    operation: MemoryOperationKind,
    status: MemoryOperationStatus,
    query: Option<String>,
    summary: String,
    detail: Option<String>,
    context_injected: bool,
}

async fn send_memory_operation_event(sess: &Session, sub_id: &str, args: MemoryOperationEventArgs) {
    let MemoryOperationEventArgs {
        source,
        operation,
        status,
        query,
        summary,
        detail,
        context_injected,
    } = args;
    sess.send_event_raw(Event {
        id: sub_id.to_string(),
        msg: EventMsg::MemoryOperation(MemoryOperationEvent {
            source,
            operation,
            status,
            query,
            summary,
            detail,
            context_injected,
        }),
    })
    .await;
}

async fn send_requires_agentmemory_backend(
    sess: &Session,
    sub_id: &str,
    operation: MemoryOperationKind,
    query: Option<String>,
) {
    send_memory_operation_event(
        sess,
        sub_id,
        MemoryOperationEventArgs {
            source: MemoryOperationSource::Human,
            operation,
            status: MemoryOperationStatus::Error,
            query,
            summary: format!(
                "{} requires agentmemory backend.",
                operation_label(operation)
            ),
            detail: None,
            context_injected: false,
        },
    )
    .await;
}

fn project_path(config: &Config) -> PathBuf {
    workspace_project(config.cwd.as_ref())
}

fn operation_label(operation: MemoryOperationKind) -> &'static str {
    match operation {
        MemoryOperationKind::Recall => "Memory recall",
        MemoryOperationKind::Remember => "Memory remember",
        MemoryOperationKind::Update => "Memory update",
        MemoryOperationKind::Drop => "Memory drop",
        MemoryOperationKind::Lessons => "Memory lessons review",
        MemoryOperationKind::Crystals => "Memory crystals review",
        MemoryOperationKind::Crystallize => "Memory crystal creation",
        MemoryOperationKind::AutoCrystallize => "Memory auto-crystallize",
        MemoryOperationKind::Insights => "Memory insights review",
        MemoryOperationKind::Reflect => "Memory reflect",
        MemoryOperationKind::Actions => "Memory actions review",
        MemoryOperationKind::ActionCreate => "Memory action creation",
        MemoryOperationKind::ActionUpdate => "Memory action update",
        MemoryOperationKind::Frontier => "Memory frontier review",
        MemoryOperationKind::Next => "Memory next review",
    }
}

fn response_success(response: &JsonValue) -> bool {
    response
        .get("success")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
}

fn response_error(response: &JsonValue) -> Option<String> {
    response
        .get("error")
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
}

fn response_count(response: &JsonValue, key: &str) -> usize {
    response
        .get(key)
        .and_then(JsonValue::as_array)
        .map_or(0, Vec::len)
}

fn response_string(response: &JsonValue, pointer: &str) -> Option<String> {
    response.pointer(pointer)?.as_str().map(ToOwned::to_owned)
}

fn response_pretty_json(response: &JsonValue) -> Option<String> {
    serde_json::to_string_pretty(response).ok()
}

fn response_detail(response: &JsonValue) -> Option<String> {
    response_error(response).or_else(|| response_pretty_json(response))
}

struct ReviewJsonResponseArgs {
    operation: MemoryOperationKind,
    query: Option<String>,
    ready_summary: String,
    empty_summary: String,
    item_count: usize,
}

fn frontier_summary(response: &JsonValue) -> String {
    let frontier_count = response_count(response, "frontier");
    let total_unblocked = response
        .get("totalUnblocked")
        .and_then(JsonValue::as_u64)
        .unwrap_or(frontier_count as u64);
    if frontier_count == 0 {
        "No unblocked frontier actions were found.".to_string()
    } else {
        format!(
            "Reviewed {frontier_count} frontier suggestions ({total_unblocked} unblocked actions total)."
        )
    }
}

fn next_summary(response: &JsonValue) -> String {
    response
        .get("message")
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "Reviewed the next suggested action.".to_string())
}

async fn remember_response(sess: &Session, sub_id: &str, response: JsonValue, content: &str) {
    let detail = response_pretty_json(&response).or_else(|| Some(content.to_string()));
    if response_success(&response) {
        let memory_id = response_string(&response, "/memory/id")
            .map(|memory_id| format!("Saved durable memory `{memory_id}`."))
            .unwrap_or_else(|| "Saved durable memory.".to_string());
        send_memory_operation_event(
            sess,
            sub_id,
            MemoryOperationEventArgs {
                source: MemoryOperationSource::Human,
                operation: MemoryOperationKind::Remember,
                status: MemoryOperationStatus::Ready,
                query: None,
                summary: memory_id,
                detail,
                context_injected: false,
            },
        )
        .await;
        return;
    }

    send_memory_operation_event(
        sess,
        sub_id,
        MemoryOperationEventArgs {
            source: MemoryOperationSource::Human,
            operation: MemoryOperationKind::Remember,
            status: MemoryOperationStatus::Error,
            query: None,
            summary: "Memory remember failed.".to_string(),
            detail,
            context_injected: false,
        },
    )
    .await;
}

async fn review_json_response(
    sess: &Session,
    sub_id: &str,
    response: JsonValue,
    args: ReviewJsonResponseArgs,
) {
    let ReviewJsonResponseArgs {
        operation,
        query,
        ready_summary,
        empty_summary,
        item_count,
    } = args;
    let status = if item_count > 0 {
        MemoryOperationStatus::Ready
    } else {
        MemoryOperationStatus::Empty
    };
    let summary = if item_count > 0 {
        ready_summary
    } else {
        empty_summary
    };
    send_memory_operation_event(
        sess,
        sub_id,
        MemoryOperationEventArgs {
            source: MemoryOperationSource::Human,
            operation,
            status,
            query,
            summary,
            detail: response_pretty_json(&response),
            context_injected: false,
        },
    )
    .await;
}

pub(crate) async fn drop_memories(sess: &Arc<Session>, config: &Arc<Config>, sub_id: String) {
    if config.memories.backend == crate::config::types::MemoryBackend::Agentmemory {
        let adapter = AgentmemoryAdapter::new();
        if let Err(e) = adapter.drop_memories(&config.memories).await {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Drop,
                    status: MemoryOperationStatus::Error,
                    query: None,
                    summary: "Memory drop failed.".to_string(),
                    detail: Some(e.to_string()),
                    context_injected: false,
                },
            )
            .await;
            return;
        }
        send_memory_operation_event(
            sess,
            &sub_id,
            MemoryOperationEventArgs {
                source: MemoryOperationSource::Human,
                operation: MemoryOperationKind::Drop,
                status: MemoryOperationStatus::Ready,
                query: None,
                summary: "Cleared Agentmemory contents.".to_string(),
                detail: None,
                context_injected: false,
            },
        )
        .await;
        return;
    }

    let mut errors = Vec::new();

    if let Some(state_db) = sess.services.state_db.as_deref() {
        if let Err(err) = state_db.clear_memory_data().await {
            errors.push(format!("failed clearing memory rows from state db: {err}"));
        }
    } else {
        errors.push("state db unavailable; memory rows were not cleared".to_string());
    }

    let memory_root = crate::memories::memory_root(&config.codex_home);
    if let Err(err) = crate::memories::clear_memory_root_contents(&memory_root).await {
        errors.push(format!(
            "failed clearing memory directory {}: {err}",
            memory_root.display()
        ));
    }

    if errors.is_empty() {
        send_memory_operation_event(
            sess,
            &sub_id,
            MemoryOperationEventArgs {
                source: MemoryOperationSource::Human,
                operation: MemoryOperationKind::Drop,
                status: MemoryOperationStatus::Ready,
                query: None,
                summary: "Dropped stored memories for this workspace.".to_string(),
                detail: Some(format!(
                    "Cleared memory rows from the state db and removed stored memory files at {}.",
                    memory_root.display()
                )),
                context_injected: false,
            },
        )
        .await;
        return;
    }

    send_memory_operation_event(
        sess,
        &sub_id,
        MemoryOperationEventArgs {
            source: MemoryOperationSource::Human,
            operation: MemoryOperationKind::Drop,
            status: MemoryOperationStatus::Error,
            query: None,
            summary: "Memory drop completed with errors.".to_string(),
            detail: Some(errors.join("; ")),
            context_injected: false,
        },
    )
    .await;
}

pub(crate) async fn update_memories(sess: &Arc<Session>, config: &Arc<Config>, sub_id: String) {
    let session_source = {
        let state = sess.state.lock().await;
        state.session_configuration.session_source.clone()
    };

    if config.memories.backend == crate::config::types::MemoryBackend::Agentmemory {
        let adapter = AgentmemoryAdapter::new();
        let result = match adapter.update_memories(&config.memories).await {
            Ok(result) => result,
            Err(e) => {
                send_memory_operation_event(
                    sess,
                    &sub_id,
                    MemoryOperationEventArgs {
                        source: MemoryOperationSource::Human,
                        operation: MemoryOperationKind::Update,
                        status: MemoryOperationStatus::Error,
                        query: None,
                        summary: "Memory update failed.".to_string(),
                        detail: Some(e.to_string()),
                        context_injected: false,
                    },
                )
                .await;
                return;
            }
        };
        let status = if result.consolidated > 0 {
            MemoryOperationStatus::Ready
        } else {
            MemoryOperationStatus::Empty
        };
        let summary = if result.consolidated > 0 {
            format!("Consolidated {} new memory entries.", result.consolidated)
        } else if result.reason.as_deref() == Some("insufficient_observations") {
            "Not enough observations yet to consolidate agentmemory.".to_string()
        } else {
            "Memory update completed without new consolidated memories.".to_string()
        };
        let detail = match (
            result.reason.as_deref(),
            result.scanned_sessions,
            result.total_observations,
        ) {
            (Some(reason), Some(scanned_sessions), Some(total_observations)) => Some(format!(
                "Agentmemory returned reason '{reason}' after scanning {scanned_sessions} sessions and considering {total_observations} candidate observations."
            )),
            (Some(reason), _, _) => Some(format!("Agentmemory returned reason '{reason}'.")),
            (None, Some(scanned_sessions), Some(total_observations)) => Some(format!(
                "Agentmemory scanned {scanned_sessions} sessions and considered {total_observations} candidate observations."
            )),
            _ => None,
        };
        send_memory_operation_event(
            sess,
            &sub_id,
            MemoryOperationEventArgs {
                source: MemoryOperationSource::Human,
                operation: MemoryOperationKind::Update,
                status,
                query: None,
                summary,
                detail,
                context_injected: false,
            },
        )
        .await;
        return;
    }

    if config.memories.backend == crate::config::types::MemoryBackend::Native {
        crate::memories::start_memories_startup_task(sess, Arc::clone(config), &session_source);
    }

    send_memory_operation_event(
        sess,
        &sub_id,
        MemoryOperationEventArgs {
            source: MemoryOperationSource::Human,
            operation: MemoryOperationKind::Update,
            status: MemoryOperationStatus::Ready,
            query: None,
            summary: "Memory update triggered.".to_string(),
            detail: Some("Consolidation is running in the background.".to_string()),
            context_injected: false,
        },
    )
    .await;
}

pub(crate) async fn recall_memories(
    sess: &Arc<Session>,
    config: &Arc<Config>,
    sub_id: String,
    query: Option<String>,
) {
    if config.memories.backend != crate::config::types::MemoryBackend::Agentmemory {
        send_requires_agentmemory_backend(sess, &sub_id, MemoryOperationKind::Recall, query).await;
        return;
    }

    let adapter = AgentmemoryAdapter::new();
    let session_id = sess.conversation_id.to_string();

    match adapter
        .recall_for_runtime(
            &session_id,
            config.cwd.as_ref(),
            query.as_deref(),
            &config.memories,
        )
        .await
    {
        Ok(result) if result.recalled => {
            let context = result.context;
            let turn_context = sess.new_default_turn_with_sub_id(sub_id.clone()).await;
            let message: ResponseItem = DeveloperInstructions::new(format!(
                "<agentmemory-recall>\n{context}\n</agentmemory-recall>"
            ))
            .into();
            sess.record_conversation_items(&turn_context, std::slice::from_ref(&message))
                .await;
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Recall,
                    status: MemoryOperationStatus::Ready,
                    query,
                    summary: "Recalled memory context and injected it into the current thread."
                        .to_string(),
                    detail: Some(context),
                    context_injected: true,
                },
            )
            .await;
        }
        Ok(_) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Recall,
                    status: MemoryOperationStatus::Empty,
                    query,
                    summary: "No relevant memory context was found.".to_string(),
                    detail: None,
                    context_injected: false,
                },
            )
            .await;
        }
        Err(e) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Recall,
                    status: MemoryOperationStatus::Error,
                    query,
                    summary: "Memory recall failed.".to_string(),
                    detail: Some(e.to_string()),
                    context_injected: false,
                },
            )
            .await;
        }
    }
}

pub(crate) async fn remember_memories(
    sess: &Arc<Session>,
    config: &Arc<Config>,
    sub_id: String,
    content: String,
) {
    if config.memories.backend != crate::config::types::MemoryBackend::Agentmemory {
        send_requires_agentmemory_backend(sess, &sub_id, MemoryOperationKind::Remember, None).await;
        return;
    }

    let adapter = AgentmemoryAdapter::new();
    match adapter.remember_memory(&content, &config.memories).await {
        Ok(response) => remember_response(sess, &sub_id, response, &content).await,
        Err(err) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Remember,
                    status: MemoryOperationStatus::Error,
                    query: None,
                    summary: "Memory remember failed.".to_string(),
                    detail: Some(err),
                    context_injected: false,
                },
            )
            .await;
        }
    }
}

pub(crate) async fn review_lessons(
    sess: &Arc<Session>,
    config: &Arc<Config>,
    sub_id: String,
    query: Option<String>,
) {
    if config.memories.backend != crate::config::types::MemoryBackend::Agentmemory {
        send_requires_agentmemory_backend(sess, &sub_id, MemoryOperationKind::Lessons, query).await;
        return;
    }

    let adapter = AgentmemoryAdapter::new();
    let project = project_path(config);
    let response = match query.as_deref() {
        Some(query) => {
            adapter
                .search_lessons(query, project.as_path(), &config.memories)
                .await
        }
        None => {
            adapter
                .list_lessons(project.as_path(), &config.memories)
                .await
        }
    };
    match response {
        Ok(response) => {
            let count = response_count(&response, "lessons");
            let ready_summary = query
                .as_ref()
                .map(|query| format!("Found {count} lessons matching `{query}`."))
                .unwrap_or_else(|| format!("Reviewed {count} lessons."));
            let empty_summary = query
                .as_ref()
                .map(|query| format!("No lessons matched `{query}`."))
                .unwrap_or_else(|| "No lessons are available yet.".to_string());
            review_json_response(
                sess,
                &sub_id,
                response,
                ReviewJsonResponseArgs {
                    operation: MemoryOperationKind::Lessons,
                    query,
                    ready_summary,
                    empty_summary,
                    item_count: count,
                },
            )
            .await;
        }
        Err(err) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Lessons,
                    status: MemoryOperationStatus::Error,
                    query,
                    summary: "Lessons review failed.".to_string(),
                    detail: Some(err),
                    context_injected: false,
                },
            )
            .await;
        }
    }
}

pub(crate) async fn review_crystals(sess: &Arc<Session>, config: &Arc<Config>, sub_id: String) {
    if config.memories.backend != crate::config::types::MemoryBackend::Agentmemory {
        send_requires_agentmemory_backend(sess, &sub_id, MemoryOperationKind::Crystals, None).await;
        return;
    }

    let adapter = AgentmemoryAdapter::new();
    let project = project_path(config);
    match adapter
        .list_crystals(project.as_path(), &config.memories)
        .await
    {
        Ok(response) => {
            let count = response_count(&response, "crystals");
            review_json_response(
                sess,
                &sub_id,
                response,
                ReviewJsonResponseArgs {
                    operation: MemoryOperationKind::Crystals,
                    query: None,
                    ready_summary: format!("Reviewed {count} crystals."),
                    empty_summary: "No crystals are available yet.".to_string(),
                    item_count: count,
                },
            )
            .await;
        }
        Err(err) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Crystals,
                    status: MemoryOperationStatus::Error,
                    query: None,
                    summary: "Crystal review failed.".to_string(),
                    detail: Some(err),
                    context_injected: false,
                },
            )
            .await;
        }
    }
}

pub(crate) async fn create_crystals(
    sess: &Arc<Session>,
    config: &Arc<Config>,
    sub_id: String,
    action_ids: Vec<String>,
) {
    if config.memories.backend != crate::config::types::MemoryBackend::Agentmemory {
        send_requires_agentmemory_backend(sess, &sub_id, MemoryOperationKind::Crystallize, None)
            .await;
        return;
    }

    let adapter = AgentmemoryAdapter::new();
    let project = project_path(config);
    match adapter
        .create_crystals(
            action_ids.as_slice(),
            &sess.conversation_id.to_string(),
            project.as_path(),
            &config.memories,
        )
        .await
    {
        Ok(response) if response_success(&response) => {
            let crystal_id = response_string(&response, "/crystal/id")
                .map(|crystal_id| format!("Created crystal `{crystal_id}`."))
                .unwrap_or_else(|| "Created crystal.".to_string());
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Crystallize,
                    status: MemoryOperationStatus::Ready,
                    query: None,
                    summary: crystal_id,
                    detail: response_pretty_json(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Ok(response) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Crystallize,
                    status: MemoryOperationStatus::Error,
                    query: None,
                    summary: "Crystal creation failed.".to_string(),
                    detail: response_detail(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Err(err) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Crystallize,
                    status: MemoryOperationStatus::Error,
                    query: None,
                    summary: "Crystal creation failed.".to_string(),
                    detail: Some(err),
                    context_injected: false,
                },
            )
            .await;
        }
    }
}

pub(crate) async fn auto_crystallize(
    sess: &Arc<Session>,
    config: &Arc<Config>,
    sub_id: String,
    older_than_days: Option<u32>,
) {
    if config.memories.backend != crate::config::types::MemoryBackend::Agentmemory {
        send_requires_agentmemory_backend(
            sess,
            &sub_id,
            MemoryOperationKind::AutoCrystallize,
            None,
        )
        .await;
        return;
    }

    let adapter = AgentmemoryAdapter::new();
    let project = project_path(config);
    let query = older_than_days.map(|value| value.to_string());
    match adapter
        .auto_crystallize(older_than_days, Some(project.as_path()), &config.memories)
        .await
    {
        Ok(response) if response_success(&response) => {
            let count = response
                .get("crystalIds")
                .and_then(JsonValue::as_array)
                .map_or(0, Vec::len);
            let status = if count > 0 {
                MemoryOperationStatus::Ready
            } else {
                MemoryOperationStatus::Empty
            };
            let summary = if count > 0 {
                format!("Auto-crystallized {count} action groups.")
            } else {
                "Auto-crystallize completed without new crystals.".to_string()
            };
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::AutoCrystallize,
                    status,
                    query: query.clone(),
                    summary,
                    detail: response_pretty_json(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Ok(response) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::AutoCrystallize,
                    status: MemoryOperationStatus::Error,
                    query: query.clone(),
                    summary: "Auto-crystallize failed.".to_string(),
                    detail: response_detail(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Err(err) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::AutoCrystallize,
                    status: MemoryOperationStatus::Error,
                    query,
                    summary: "Auto-crystallize failed.".to_string(),
                    detail: Some(err),
                    context_injected: false,
                },
            )
            .await;
        }
    }
}

pub(crate) async fn reflect_memories(
    sess: &Arc<Session>,
    config: &Arc<Config>,
    sub_id: String,
    max_clusters: Option<u32>,
) {
    if config.memories.backend != crate::config::types::MemoryBackend::Agentmemory {
        send_requires_agentmemory_backend(sess, &sub_id, MemoryOperationKind::Reflect, None).await;
        return;
    }

    let adapter = AgentmemoryAdapter::new();
    let project = project_path(config);
    let query = max_clusters.map(|value| value.to_string());
    match adapter
        .reflect(project.as_path(), max_clusters, &config.memories)
        .await
    {
        Ok(response) if response_success(&response) => {
            let new_insights = response
                .get("newInsights")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0);
            let reinforced = response
                .get("reinforced")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0);
            let status = if new_insights > 0 || reinforced > 0 {
                MemoryOperationStatus::Ready
            } else {
                MemoryOperationStatus::Empty
            };
            let summary = format!(
                "Reflection completed with {new_insights} new insights and {reinforced} reinforcements."
            );
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Reflect,
                    status,
                    query: query.clone(),
                    summary,
                    detail: response_pretty_json(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Ok(response) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Reflect,
                    status: MemoryOperationStatus::Error,
                    query: query.clone(),
                    summary: "Memory reflect failed.".to_string(),
                    detail: response_detail(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Err(err) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Reflect,
                    status: MemoryOperationStatus::Error,
                    query,
                    summary: "Memory reflect failed.".to_string(),
                    detail: Some(err),
                    context_injected: false,
                },
            )
            .await;
        }
    }
}

pub(crate) async fn review_insights(
    sess: &Arc<Session>,
    config: &Arc<Config>,
    sub_id: String,
    query: Option<String>,
) {
    if config.memories.backend != crate::config::types::MemoryBackend::Agentmemory {
        send_requires_agentmemory_backend(sess, &sub_id, MemoryOperationKind::Insights, query)
            .await;
        return;
    }

    let adapter = AgentmemoryAdapter::new();
    let project = project_path(config);
    let response = match query.as_deref() {
        Some(query) => {
            adapter
                .search_insights(query, project.as_path(), &config.memories)
                .await
        }
        None => {
            adapter
                .list_insights(project.as_path(), &config.memories)
                .await
        }
    };
    match response {
        Ok(response) => {
            let count = response_count(&response, "insights");
            let ready_summary = query
                .as_ref()
                .map(|query| format!("Found {count} insights matching `{query}`."))
                .unwrap_or_else(|| format!("Reviewed {count} insights."));
            let empty_summary = query
                .as_ref()
                .map(|query| format!("No insights matched `{query}`."))
                .unwrap_or_else(|| "No insights are available yet.".to_string());
            review_json_response(
                sess,
                &sub_id,
                response,
                ReviewJsonResponseArgs {
                    operation: MemoryOperationKind::Insights,
                    query,
                    ready_summary,
                    empty_summary,
                    item_count: count,
                },
            )
            .await;
        }
        Err(err) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Insights,
                    status: MemoryOperationStatus::Error,
                    query,
                    summary: "Insights review failed.".to_string(),
                    detail: Some(err),
                    context_injected: false,
                },
            )
            .await;
        }
    }
}

pub(crate) async fn list_actions(
    sess: &Arc<Session>,
    config: &Arc<Config>,
    sub_id: String,
    status: Option<String>,
) {
    if config.memories.backend != crate::config::types::MemoryBackend::Agentmemory {
        send_requires_agentmemory_backend(sess, &sub_id, MemoryOperationKind::Actions, status)
            .await;
        return;
    }

    let adapter = AgentmemoryAdapter::new();
    let project = project_path(config);
    match adapter
        .list_actions(project.as_path(), status.as_deref(), &config.memories)
        .await
    {
        Ok(response) => {
            let count = response_count(&response, "actions");
            let ready_summary = status
                .as_ref()
                .map(|status| format!("Reviewed {count} `{status}` actions."))
                .unwrap_or_else(|| format!("Reviewed {count} actions."));
            let empty_summary = status
                .as_ref()
                .map(|status| format!("No `{status}` actions were found."))
                .unwrap_or_else(|| "No actions are available yet.".to_string());
            review_json_response(
                sess,
                &sub_id,
                response,
                ReviewJsonResponseArgs {
                    operation: MemoryOperationKind::Actions,
                    query: status,
                    ready_summary,
                    empty_summary,
                    item_count: count,
                },
            )
            .await;
        }
        Err(err) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Actions,
                    status: MemoryOperationStatus::Error,
                    query: status,
                    summary: "Action review failed.".to_string(),
                    detail: Some(err),
                    context_injected: false,
                },
            )
            .await;
        }
    }
}

pub(crate) async fn create_action(
    sess: &Arc<Session>,
    config: &Arc<Config>,
    sub_id: String,
    title: String,
) {
    if config.memories.backend != crate::config::types::MemoryBackend::Agentmemory {
        send_requires_agentmemory_backend(sess, &sub_id, MemoryOperationKind::ActionCreate, None)
            .await;
        return;
    }

    let adapter = AgentmemoryAdapter::new();
    let project = project_path(config);
    let created_by = format!("codex:{}", sess.conversation_id);
    match adapter
        .create_action(&title, &created_by, project.as_path(), &config.memories)
        .await
    {
        Ok(response) if response_success(&response) => {
            let action_id = response_string(&response, "/action/id")
                .map(|action_id| format!("Created action `{action_id}`."))
                .unwrap_or_else(|| "Created action.".to_string());
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::ActionCreate,
                    status: MemoryOperationStatus::Ready,
                    query: None,
                    summary: action_id,
                    detail: response_pretty_json(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Ok(response) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::ActionCreate,
                    status: MemoryOperationStatus::Error,
                    query: None,
                    summary: "Action creation failed.".to_string(),
                    detail: response_detail(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Err(err) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::ActionCreate,
                    status: MemoryOperationStatus::Error,
                    query: None,
                    summary: "Action creation failed.".to_string(),
                    detail: Some(err),
                    context_injected: false,
                },
            )
            .await;
        }
    }
}

pub(crate) async fn update_action(
    sess: &Arc<Session>,
    config: &Arc<Config>,
    sub_id: String,
    action_id: String,
    status: String,
) {
    if config.memories.backend != crate::config::types::MemoryBackend::Agentmemory {
        send_requires_agentmemory_backend(sess, &sub_id, MemoryOperationKind::ActionUpdate, None)
            .await;
        return;
    }

    let adapter = AgentmemoryAdapter::new();
    match adapter
        .update_action(&action_id, &status, &config.memories)
        .await
    {
        Ok(response) if response_success(&response) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::ActionUpdate,
                    status: MemoryOperationStatus::Ready,
                    query: None,
                    summary: format!("Updated action `{action_id}` to `{status}`."),
                    detail: response_pretty_json(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Ok(response) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::ActionUpdate,
                    status: MemoryOperationStatus::Error,
                    query: None,
                    summary: "Action update failed.".to_string(),
                    detail: response_detail(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Err(err) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::ActionUpdate,
                    status: MemoryOperationStatus::Error,
                    query: None,
                    summary: "Action update failed.".to_string(),
                    detail: Some(err),
                    context_injected: false,
                },
            )
            .await;
        }
    }
}

pub(crate) async fn review_frontier(
    sess: &Arc<Session>,
    config: &Arc<Config>,
    sub_id: String,
    limit: Option<u32>,
) {
    if config.memories.backend != crate::config::types::MemoryBackend::Agentmemory {
        send_requires_agentmemory_backend(sess, &sub_id, MemoryOperationKind::Frontier, None).await;
        return;
    }

    let adapter = AgentmemoryAdapter::new();
    let project = project_path(config);
    let query = limit.map(|value| value.to_string());
    let agent_id = sess.conversation_id.to_string();
    match adapter
        .frontier(
            project.as_path(),
            Some(agent_id.as_str()),
            limit,
            &config.memories,
        )
        .await
    {
        Ok(response) if response_success(&response) => {
            let count = response_count(&response, "frontier");
            let status = if count > 0 {
                MemoryOperationStatus::Ready
            } else {
                MemoryOperationStatus::Empty
            };
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Frontier,
                    status,
                    query: query.clone(),
                    summary: frontier_summary(&response),
                    detail: response_pretty_json(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Ok(response) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Frontier,
                    status: MemoryOperationStatus::Error,
                    query: query.clone(),
                    summary: "Frontier review failed.".to_string(),
                    detail: response_detail(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Err(err) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Frontier,
                    status: MemoryOperationStatus::Error,
                    query,
                    summary: "Frontier review failed.".to_string(),
                    detail: Some(err),
                    context_injected: false,
                },
            )
            .await;
        }
    }
}

pub(crate) async fn review_next(sess: &Arc<Session>, config: &Arc<Config>, sub_id: String) {
    if config.memories.backend != crate::config::types::MemoryBackend::Agentmemory {
        send_requires_agentmemory_backend(sess, &sub_id, MemoryOperationKind::Next, None).await;
        return;
    }

    let adapter = AgentmemoryAdapter::new();
    let project = project_path(config);
    let agent_id = sess.conversation_id.to_string();
    match adapter
        .next_action(project.as_path(), Some(agent_id.as_str()), &config.memories)
        .await
    {
        Ok(response) if response_success(&response) => {
            let status = if response.get("suggestion").is_some() {
                MemoryOperationStatus::Ready
            } else {
                MemoryOperationStatus::Empty
            };
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Next,
                    status,
                    query: None,
                    summary: next_summary(&response),
                    detail: response_pretty_json(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Ok(response) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Next,
                    status: MemoryOperationStatus::Error,
                    query: None,
                    summary: "Next-action review failed.".to_string(),
                    detail: response_detail(&response),
                    context_injected: false,
                },
            )
            .await;
        }
        Err(err) => {
            send_memory_operation_event(
                sess,
                &sub_id,
                MemoryOperationEventArgs {
                    source: MemoryOperationSource::Human,
                    operation: MemoryOperationKind::Next,
                    status: MemoryOperationStatus::Error,
                    query: None,
                    summary: "Next-action review failed.".to_string(),
                    detail: Some(err),
                    context_injected: false,
                },
            )
            .await;
        }
    }
}
