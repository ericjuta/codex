//! Agentmemory integration adapter.
//!
//! This module provides the seam for integrating the `agentmemory` service
//! as a replacement for Codex's native memory engine.

pub(crate) mod context_planner;
mod observe_payload;
pub(crate) mod retrieval_trace;

use crate::agentmemory::observe_payload::build_observe_payload;
use crate::agentmemory::retrieval_trace::AgentmemoryRetrievalTrace;
use crate::agentmemory::retrieval_trace::AgentmemoryRetrievalTraceSummary;
use crate::config::types::AgentmemoryConfig;
use crate::config::types::MemoriesConfig;
use codex_git_utils::get_git_repo_root;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

/// A placeholder adapter struct for agentmemory integration.
#[derive(Debug, Default, Clone)]
pub struct AgentmemoryAdapter {
    // Configuration and state will be added here in subsequent PRs.
}

/// A shared, pooled HTTP client for agentmemory interactions.
/// Reusing the client allows connection pooling (keep-alive) for high throughput.
static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

pub(crate) const DEFAULT_RUNTIME_RECALL_TOKEN_BUDGET: usize =
    context_planner::DEFAULT_CONTEXT_BUDGET_TOKENS;
const MEMORY_RUNTIME_DEVELOPER_INSTRUCTIONS: &str = "Use `memory_recall` for prior work, earlier decisions, previous failures, resumed threads, or other historical context that is not already present in the current thread.\n\
     Use `memory_remember` only for durable, high-value knowledge that should survive beyond the current turn.\n\
     Use `memory_lessons`, `memory_crystals`, `memory_insights`, `memory_actions`, `memory_missions`, `memory_handoffs`, `memory_handoff_generate`, `memory_branch_overlays`, `memory_guardrails`, `memory_decisions`, `memory_dossiers`, `memory_routine_candidates`, `memory_frontier`, and `memory_next` as read-oriented agentmemory review surfaces when they would materially help with coordination or retrieval.\n\
     Agentmemory startup context may be attached below when available.\n\
     Assistant `memory_recall` stays turn-local unless it explicitly passes `scope: \"thread\"`.\n\
     Prefer targeted queries naming the feature, file, bug, or decision you need.\n\
     If the current runtime exposes tools through a wrapper surface (for example, `exec` with nested `tools`), treat the callable nested tool surface as authoritative when checking whether these memory tools are available.\n\
     Do not call memory tools on every turn; first use the current thread context, then reach for agentmemory when that context appears insufficient.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryRecallResult {
    pub(crate) recalled: bool,
    pub(crate) context: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub(crate) struct AgentmemoryConsolidateResult {
    pub(crate) consolidated: usize,
    pub(crate) reason: Option<String>,
    #[serde(rename = "scannedSessions")]
    pub(crate) scanned_sessions: Option<usize>,
    #[serde(rename = "totalObservations")]
    pub(crate) total_observations: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub(crate) struct AgentmemorySummarizeResult {
    #[serde(default)]
    pub(crate) success: bool,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub(crate) struct AgentmemoryContextResult {
    #[serde(default)]
    pub(crate) context: String,
    #[serde(default)]
    pub(crate) skipped: bool,
    #[serde(default)]
    pub(crate) trace: Option<AgentmemoryRetrievalTrace>,
}

impl AgentmemoryContextResult {
    pub(crate) fn retrieval_trace_summary(&self) -> Option<AgentmemoryRetrievalTraceSummary> {
        self.trace
            .as_ref()
            .map(AgentmemoryRetrievalTraceSummary::from)
    }
}

fn get_client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| reqwest::Client::builder().build().unwrap_or_default())
}

fn parse_bool_override(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

pub(crate) fn workspace_project(cwd: &Path) -> PathBuf {
    get_git_repo_root(cwd).unwrap_or_else(|| cwd.to_path_buf())
}

impl AgentmemoryAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    fn api_base(&self, memories: &MemoriesConfig) -> String {
        if let Ok(url) = std::env::var("AGENTMEMORY_URL")
            && !url.trim().is_empty()
        {
            return url;
        }

        let default_base_url = AgentmemoryConfig::default().base_url;
        if memories.agentmemory.base_url != default_base_url {
            return memories.agentmemory.base_url.clone();
        }

        std::env::var("III_REST_PORT")
            .map(|port| format!("http://127.0.0.1:{port}"))
            .unwrap_or(default_base_url)
    }

    pub(crate) fn inject_context_enabled(&self, memories: &MemoriesConfig) -> bool {
        std::env::var("AGENTMEMORY_INJECT_CONTEXT")
            .ok()
            .as_deref()
            .and_then(parse_bool_override)
            .unwrap_or(memories.agentmemory.inject_context)
    }

    pub(crate) fn consolidation_enabled(&self, memories: &MemoriesConfig) -> bool {
        std::env::var("CONSOLIDATION_ENABLED")
            .ok()
            .as_deref()
            .and_then(parse_bool_override)
            .unwrap_or(memories.use_memories)
    }

    fn auth_secret(&self, memories: &MemoriesConfig) -> Option<String> {
        if let Ok(secret) = std::env::var("AGENTMEMORY_SECRET")
            && !secret.trim().is_empty()
        {
            return Some(secret);
        }

        std::env::var(&memories.agentmemory.secret_env_var)
            .ok()
            .filter(|secret| !secret.trim().is_empty())
    }

    fn request_builder(
        &self,
        method: reqwest::Method,
        url: &str,
        memories: &MemoriesConfig,
    ) -> reqwest::RequestBuilder {
        let builder = get_client().request(method, url);
        if let Some(secret) = self.auth_secret(memories) {
            builder.bearer_auth(secret)
        } else {
            builder
        }
    }

    async fn json_or_error(response: reqwest::Response) -> Result<JsonValue, String> {
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let detail = if body.trim().is_empty() {
                String::new()
            } else {
                format!(": {body}")
            };
            return Err(format!("request failed with status {status}{detail}"));
        }
        response
            .json::<JsonValue>()
            .await
            .map_err(|err| err.to_string())
    }

    async fn ensure_success(response: reqwest::Response) -> Result<(), String> {
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let detail = if body.trim().is_empty() {
                String::new()
            } else {
                format!(": {body}")
            };
            return Err(format!("request failed with status {status}{detail}"));
        }
        Ok(())
    }

    async fn parse_context_result(
        response: reqwest::Response,
    ) -> Result<AgentmemoryContextResult, String> {
        let payload = Self::json_or_error(response).await?;
        serde_json::from_value(payload).map_err(|err| err.to_string())
    }

    /// Builds the developer instructions for the assistant-facing memory recall
    /// tool when the `agentmemory` backend is active.
    pub async fn build_startup_developer_instructions(
        &self,
        _codex_home: &Path,
        _token_budget: usize,
    ) -> Option<String> {
        Some(MEMORY_RUNTIME_DEVELOPER_INSTRUCTIONS.to_string())
    }

    /// Attempts to parse a tool command string as JSON to recover structured
    /// arguments. Falls back to the original string value on parse failure.
    fn parse_structured_tool_input(raw: &serde_json::Value) -> serde_json::Value {
        if let Some(s) = raw.as_str()
            && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s)
        {
            return parsed;
        }
        raw.clone()
    }

    /// Extracts file paths and search terms from structured tool arguments
    /// so that Agentmemory observations mention the relevant paths and queries.
    fn extract_file_enrichment(tool_input: &serde_json::Value) -> (Vec<String>, Vec<String>) {
        let mut files: Vec<String> = Vec::new();
        let mut search_terms: Vec<String> = Vec::new();
        let mut seen_files = HashSet::new();
        let mut seen_terms = HashSet::new();

        if let Some(obj) = tool_input.as_object() {
            // File path fields
            for key in &["file_path", "path", "dir_path"] {
                if let Some(v) = obj.get(*key).and_then(|v| v.as_str())
                    && !v.is_empty()
                    && seen_files.insert(v.to_string())
                {
                    files.push(v.to_string());
                }
            }
            // Array of paths
            if let Some(arr) = obj.get("paths").and_then(|v| v.as_array()) {
                for item in arr {
                    if let Some(s) = item.as_str()
                        && !s.is_empty()
                        && seen_files.insert(s.to_string())
                    {
                        files.push(s.to_string());
                    }
                }
            }
            for key in &["pattern", "query", "term", "search_term"] {
                if let Some(value) = obj.get(*key).and_then(|value| value.as_str())
                    && !value.is_empty()
                    && seen_terms.insert(value.to_string())
                {
                    search_terms.push(value.to_string());
                }
            }
            if let Some(arr) = obj.get("terms").and_then(|value| value.as_array()) {
                for item in arr {
                    if let Some(term) = item.as_str()
                        && !term.is_empty()
                        && seen_terms.insert(term.to_string())
                    {
                        search_terms.push(term.to_string());
                    }
                }
            }
        }
        (files, search_terms)
    }

    /// Asynchronously captures and stores lifecycle events in `agentmemory`.
    ///
    /// This method allows Codex hooks (like `SessionStart`, `PostToolUse`) to
    /// be transmitted without blocking the hot path of the shell or model output.
    pub async fn capture_event(
        &self,
        event_name: &str,
        payload_json: serde_json::Value,
        memories: &MemoriesConfig,
    ) {
        let url = format!("{}/agentmemory/observe", self.api_base(memories));
        let body = match build_observe_payload(event_name, payload_json) {
            Ok(body) => body,
            Err(err) => {
                tracing::warn!(
                    "Agentmemory observation skipped for unsupported or invalid {} payload: {}",
                    event_name,
                    err
                );
                return;
            }
        };

        match self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
        {
            Ok(response) => {
                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    tracing::warn!(
                        "Agentmemory observation failed: {} event to {} returned {}: {}",
                        event_name,
                        url,
                        status,
                        body
                    );
                }
            }
            Err(e) => {
                // Log a warning instead of failing silently. This won't crash the session,
                // but will alert developers that memory observation is degraded.
                tracing::warn!(
                    "Agentmemory observation failed: could not send {} event to {}: {}",
                    event_name,
                    url,
                    e
                );
            }
        }
    }

    /// Retrieves memory context mid-session via `agentmemory`'s hybrid search.
    ///
    /// Unlike `build_startup_developer_instructions`, this uses the real
    /// session ID and an optional query to scope retrieval.
    pub(crate) async fn recall_context_result(
        &self,
        session_id: &str,
        cwd: &Path,
        query: Option<&str>,
        token_budget: usize,
        memories: &MemoriesConfig,
    ) -> Result<AgentmemoryContextResult, String> {
        let url = format!("{}/agentmemory/context", self.api_base(memories));
        let project = workspace_project(cwd);

        let mut body = json!({
            "sessionId": session_id,
            "project": project.to_string_lossy(),
            "budget": token_budget,
        });
        if let Some(q) = query {
            body["query"] = serde_json::Value::String(q.to_string());
        }

        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::parse_context_result(res).await
    }

    pub async fn recall_context(
        &self,
        session_id: &str,
        cwd: &Path,
        query: Option<&str>,
        token_budget: usize,
        memories: &MemoriesConfig,
    ) -> Result<String, String> {
        self.recall_context_result(session_id, cwd, query, token_budget, memories)
            .await
            .map(|payload| payload.context)
    }

    pub(crate) async fn recall_for_runtime(
        &self,
        session_id: &str,
        cwd: &Path,
        query: Option<&str>,
        memories: &MemoriesConfig,
    ) -> Result<MemoryRecallResult, String> {
        let context = self
            .recall_context_result(
                session_id,
                cwd,
                query,
                DEFAULT_RUNTIME_RECALL_TOKEN_BUDGET,
                memories,
            )
            .await?;

        Ok(MemoryRecallResult {
            recalled: !context.context.trim().is_empty(),
            context: context.context,
        })
    }

    /// Registers a session so Agentmemory's session-backed views can discover it.
    pub async fn start_session(
        &self,
        session_id: &str,
        project: &Path,
        cwd: &Path,
        memories: &MemoriesConfig,
    ) -> Result<String, String> {
        let url = format!("{}/agentmemory/session/start", self.api_base(memories));
        let body = json!({
            "sessionId": session_id,
            "project": project.display().to_string(),
            "cwd": cwd.display().to_string(),
        });
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let payload = Self::parse_context_result(res).await?;
        Ok(payload.context)
    }

    pub async fn enrich_context(
        &self,
        session_id: &str,
        tool_name: &str,
        tool_input: &serde_json::Value,
        memories: &MemoriesConfig,
    ) -> Result<String, String> {
        let tool_input = Self::parse_structured_tool_input(tool_input);
        let (files, terms) = Self::extract_file_enrichment(&tool_input);
        if files.is_empty() && terms.is_empty() {
            return Ok(String::new());
        }

        let url = format!("{}/agentmemory/enrich", self.api_base(memories));
        let body = json!({
            "sessionId": session_id,
            "files": files,
            "terms": terms,
            "toolName": tool_name,
        });
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        let payload = Self::parse_context_result(res).await?;
        Ok(payload.context)
    }

    pub(crate) async fn refresh_context_result(
        &self,
        session_id: &str,
        cwd: &Path,
        query: &str,
        memories: &MemoriesConfig,
    ) -> Result<AgentmemoryContextResult, String> {
        let url = format!("{}/agentmemory/context/refresh", self.api_base(memories));
        let body = json!({
            "sessionId": session_id,
            "project": workspace_project(cwd).display().to_string(),
            "query": query,
        });
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::parse_context_result(res).await
    }

    /// Marks a session completed so Agentmemory's viewer can stop showing it as active.
    pub async fn end_session(
        &self,
        session_id: &str,
        memories: &MemoriesConfig,
    ) -> Result<(), String> {
        let url = format!("{}/agentmemory/session/end", self.api_base(memories));
        let body = json!({ "sessionId": session_id });
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::ensure_success(res).await?;
        Ok(())
    }

    pub(crate) async fn remember_memory(
        &self,
        content: &str,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/remember", self.api_base(memories));
        let body = json!({ "content": content });
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    /// Asynchronously triggers a memory refresh/update operation in `agentmemory`.
    pub(crate) async fn update_memories(
        &self,
        memories: &MemoriesConfig,
    ) -> Result<AgentmemoryConsolidateResult, String> {
        let url = format!("{}/agentmemory/consolidate", self.api_base(memories));
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let payload = Self::json_or_error(res).await?;
        serde_json::from_value::<AgentmemoryConsolidateResult>(payload)
            .map_err(|err| err.to_string())
    }

    /// Best-effort end-of-session summarization so later recalls can use durable
    /// cross-session summaries.
    pub(crate) async fn summarize_session(
        &self,
        session_id: &str,
        memories: &MemoriesConfig,
    ) -> Result<AgentmemorySummarizeResult, String> {
        let url = format!("{}/agentmemory/summarize", self.api_base(memories));
        let body = json!({ "sessionId": session_id });
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let payload = Self::json_or_error(res).await?;
        serde_json::from_value::<AgentmemorySummarizeResult>(payload).map_err(|err| err.to_string())
    }

    /// Asynchronously drops/clears the memory store in `agentmemory`.
    pub async fn drop_memories(&self, memories: &MemoriesConfig) -> Result<(), String> {
        let url = format!("{}/agentmemory/forget", self.api_base(memories));
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&json!({"all": true}))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::ensure_success(res).await?;
        Ok(())
    }

    pub(crate) async fn list_lessons(
        &self,
        project: &Path,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/lessons", self.api_base(memories));
        let project = project.display().to_string();
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&[("project", project)])
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn search_lessons(
        &self,
        query: &str,
        project: &Path,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/lessons/search", self.api_base(memories));
        let body = json!({
            "query": query,
            "project": project.display().to_string(),
        });
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn list_crystals(
        &self,
        project: &Path,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/crystals", self.api_base(memories));
        let project = project.display().to_string();
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&[("project", project)])
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn create_crystals(
        &self,
        action_ids: &[String],
        session_id: &str,
        project: &Path,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/crystals/create", self.api_base(memories));
        let body = json!({
            "actionIds": action_ids,
            "sessionId": session_id,
            "project": project.display().to_string(),
        });
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn auto_crystallize(
        &self,
        older_than_days: Option<u32>,
        project: Option<&Path>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/crystals/auto", self.api_base(memories));
        let mut body = json!({});
        if let Some(older_than_days) = older_than_days {
            body["olderThanDays"] = json!(older_than_days);
        }
        if let Some(project) = project {
            body["project"] = json!(project.display().to_string());
        }
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn reflect(
        &self,
        project: &Path,
        max_clusters: Option<u32>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/reflect", self.api_base(memories));
        let mut body = json!({ "project": project.display().to_string() });
        if let Some(max_clusters) = max_clusters {
            body["maxClusters"] = json!(max_clusters);
        }
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn list_insights(
        &self,
        project: &Path,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/insights", self.api_base(memories));
        let project = project.display().to_string();
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&[("project", project)])
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn search_insights(
        &self,
        query: &str,
        project: &Path,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/insights/search", self.api_base(memories));
        let body = json!({
            "query": query,
            "project": project.display().to_string(),
        });
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn list_actions(
        &self,
        project: &Path,
        status: Option<&str>,
        owner: Option<&str>,
        limit: Option<u32>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/actions", self.api_base(memories));
        let mut query = vec![("project".to_string(), project.display().to_string())];
        if let Some(status) = status {
            query.push(("status".to_string(), status.to_string()));
        }
        if let Some(owner) = owner {
            query.push(("owner".to_string(), owner.to_string()));
        }
        if let Some(limit) = limit {
            query.push(("limit".to_string(), limit.to_string()));
        }
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&query)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn create_action(
        &self,
        title: &str,
        created_by: &str,
        project: &Path,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/actions", self.api_base(memories));
        let body = json!({
            "title": title,
            "createdBy": created_by,
            "project": project.display().to_string(),
        });
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn update_action(
        &self,
        action_id: &str,
        status: &str,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/actions/update", self.api_base(memories));
        let body = json!({
            "actionId": action_id,
            "status": status,
        });
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn frontier(
        &self,
        project: &Path,
        agent_id: Option<&str>,
        limit: Option<u32>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/frontier", self.api_base(memories));
        let mut query = vec![("project".to_string(), project.display().to_string())];
        if let Some(agent_id) = agent_id {
            query.push(("agentId".to_string(), agent_id.to_string()));
        }
        if let Some(limit) = limit {
            query.push(("limit".to_string(), limit.to_string()));
        }
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&query)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn next_action(
        &self,
        project: &Path,
        agent_id: Option<&str>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/next", self.api_base(memories));
        let mut query = vec![("project".to_string(), project.display().to_string())];
        if let Some(agent_id) = agent_id {
            query.push(("agentId".to_string(), agent_id.to_string()));
        }
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&query)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn list_missions(
        &self,
        project: &Path,
        status: Option<&str>,
        owner: Option<&str>,
        limit: Option<u32>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/missions", self.api_base(memories));
        let mut query = vec![("project".to_string(), project.display().to_string())];
        if let Some(status) = status {
            query.push(("status".to_string(), status.to_string()));
        }
        if let Some(owner) = owner {
            query.push(("owner".to_string(), owner.to_string()));
        }
        if let Some(limit) = limit {
            query.push(("limit".to_string(), limit.to_string()));
        }
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&query)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn list_branch_overlays(
        &self,
        project: &Path,
        branch: Option<&str>,
        limit: Option<u32>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/branch-overlays", self.api_base(memories));
        let mut query = vec![("project".to_string(), project.display().to_string())];
        if let Some(branch) = branch {
            query.push(("branch".to_string(), branch.to_string()));
        }
        if let Some(limit) = limit {
            query.push(("limit".to_string(), limit.to_string()));
        }
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&query)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn list_guardrails(
        &self,
        project: &Path,
        branch: Option<&str>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/guardrails", self.api_base(memories));
        let mut query = vec![("project".to_string(), project.display().to_string())];
        if let Some(branch) = branch {
            query.push(("branch".to_string(), branch.to_string()));
        }
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&query)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn search_guardrails(
        &self,
        query: &str,
        project: &Path,
        branch: Option<&str>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/guardrails/search", self.api_base(memories));
        let mut body = json!({
            "query": query,
            "project": project.display().to_string(),
        });
        if let Some(branch) = branch {
            body["branch"] = JsonValue::String(branch.to_string());
        }
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn list_decisions(
        &self,
        project: &Path,
        branch: Option<&str>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/decisions", self.api_base(memories));
        let mut query = vec![("project".to_string(), project.display().to_string())];
        if let Some(branch) = branch {
            query.push(("branch".to_string(), branch.to_string()));
        }
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&query)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn search_decisions(
        &self,
        query: &str,
        project: &Path,
        branch: Option<&str>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/decisions/search", self.api_base(memories));
        let mut body = json!({
            "query": query,
            "project": project.display().to_string(),
        });
        if let Some(branch) = branch {
            body["branch"] = JsonValue::String(branch.to_string());
        }
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn list_dossiers(
        &self,
        project: &Path,
        branch: Option<&str>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/dossiers", self.api_base(memories));
        let mut query = vec![("project".to_string(), project.display().to_string())];
        if let Some(branch) = branch {
            query.push(("branch".to_string(), branch.to_string()));
        }
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&query)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn get_dossier(
        &self,
        project: &Path,
        file_path: &str,
        branch: Option<&str>,
        refresh: bool,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/dossiers/get", self.api_base(memories));
        let mut query = vec![
            ("project".to_string(), project.display().to_string()),
            ("filePath".to_string(), file_path.to_string()),
        ];
        if let Some(branch) = branch {
            query.push(("branch".to_string(), branch.to_string()));
        }
        if refresh {
            query.push(("refresh".to_string(), "true".to_string()));
        }
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&query)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn list_routine_candidates(
        &self,
        project: &Path,
        branch: Option<&str>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/routine-candidates", self.api_base(memories));
        let mut query = vec![("project".to_string(), project.display().to_string())];
        if let Some(branch) = branch {
            query.push(("branch".to_string(), branch.to_string()));
        }
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&query)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn get_mission(
        &self,
        mission_id: &str,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!(
            "{}/agentmemory/missions/{mission_id}",
            self.api_base(memories)
        );
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn list_handoffs(
        &self,
        project: &Path,
        scope_type: Option<&str>,
        scope_id: Option<&str>,
        limit: Option<u32>,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/handoffs", self.api_base(memories));
        let mut query = vec![("project".to_string(), project.display().to_string())];
        if let Some(scope_type) = scope_type {
            query.push(("scopeType".to_string(), scope_type.to_string()));
        }
        if let Some(scope_id) = scope_id {
            query.push(("scopeId".to_string(), scope_id.to_string()));
        }
        if let Some(limit) = limit {
            query.push(("limit".to_string(), limit.to_string()));
        }
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .query(&query)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn get_handoff(
        &self,
        handoff_packet_id: &str,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!(
            "{}/agentmemory/handoffs/{handoff_packet_id}",
            self.api_base(memories)
        );
        let res = self
            .request_builder(reqwest::Method::GET, &url, memories)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn generate_handoff(
        &self,
        scope_type: &str,
        scope_id: &str,
        project: &Path,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!("{}/agentmemory/handoffs/generate", self.api_base(memories));
        let body = json!({
            "scopeType": scope_type,
            "scopeId": scope_id,
            "project": project.display().to_string(),
        });
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }

    pub(crate) async fn consolidate_pipeline(
        &self,
        memories: &MemoriesConfig,
    ) -> Result<JsonValue, String> {
        let url = format!(
            "{}/agentmemory/consolidate-pipeline",
            self.api_base(memories)
        );
        let body = json!({ "tier": "all", "force": true });
        let res = self
            .request_builder(reqwest::Method::POST, &url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        Self::json_or_error(res).await
    }
}
#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::config::types::MemoriesConfig;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::ffi::OsString;
    use std::sync::Mutex;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    use wiremock::matchers::query_param;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var_os(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(value) => unsafe {
                    std::env::set_var(self.key, value);
                },
                None => unsafe {
                    std::env::remove_var(self.key);
                },
            }
        }
    }

    fn test_memories(server: &MockServer) -> MemoriesConfig {
        let mut memories = MemoriesConfig::default();
        memories.agentmemory.base_url = server.uri();
        memories
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn test_startup_instructions_describe_current_runtime_surface() {
        let adapter = AgentmemoryAdapter::new();

        let instructions = adapter
            .build_startup_developer_instructions(Path::new("/tmp/project"), 256)
            .await
            .expect("instructions should be returned");

        assert!(instructions.contains("Use `memory_recall`"));
        assert!(instructions.contains("Use `memory_remember`"));
        assert!(instructions.contains("memory_lessons"));
        assert!(instructions.contains("prior work, earlier decisions, previous failures"));
        assert!(instructions.contains("Agentmemory startup context may be attached below"));
        assert!(instructions.contains(
            "Prefer targeted queries naming the feature, file, bug, or decision you need"
        ));
        assert!(instructions.contains("Do not call memory tools on every turn"));
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn update_memories_returns_consolidate_payload() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _url_guard = EnvVarGuard::set("AGENTMEMORY_URL", "");
        let memories = test_memories(&server);

        Mock::given(method("POST"))
            .and(path("/agentmemory/consolidate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "consolidated": 0,
                "reason": "insufficient_observations",
                "scannedSessions": 4,
                "totalObservations": 0
            })))
            .expect(1)
            .mount(&server)
            .await;

        let result = adapter
            .update_memories(&memories)
            .await
            .expect("consolidate result should parse");

        assert_eq!(
            result,
            AgentmemoryConsolidateResult {
                consolidated: 0,
                reason: Some("insufficient_observations".to_string()),
                scanned_sessions: Some(4),
                total_observations: Some(0),
            }
        );
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn start_session_uses_configured_base_url_and_returns_context() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _url_guard = EnvVarGuard::set("AGENTMEMORY_URL", "");
        let memories = test_memories(&server);

        Mock::given(method("POST"))
            .and(path("/agentmemory/session/start"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "context": "<agentmemory-context>remember this</agentmemory-context>"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let context = adapter
            .start_session(
                "session-1",
                Path::new("/tmp/project"),
                Path::new("/tmp/project"),
                &memories,
            )
            .await
            .expect("session start should succeed");

        assert_eq!(
            context,
            "<agentmemory-context>remember this</agentmemory-context>"
        );
    }

    #[test]
    #[serial_test::serial(agentmemory_env)]
    fn inject_context_env_override_beats_config() {
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _inject_guard = EnvVarGuard::set("AGENTMEMORY_INJECT_CONTEXT", "false");
        let adapter = AgentmemoryAdapter::new();
        let mut memories = MemoriesConfig::default();
        memories.agentmemory.inject_context = true;

        assert!(!adapter.inject_context_enabled(&memories));
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn direct_secret_override_beats_secret_env_var_indirection() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _direct_secret_guard = EnvVarGuard::set("AGENTMEMORY_SECRET", "direct-secret");
        let _indirect_secret_guard = EnvVarGuard::set("CODEX_AGENTMEMORY_SECRET", "indirect");
        let mut memories = test_memories(&server);
        memories.agentmemory.secret_env_var = "CODEX_AGENTMEMORY_SECRET".to_string();

        Mock::given(method("POST"))
            .and(path("/agentmemory/session/start"))
            .and(header("authorization", "Bearer direct-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "context": "" })))
            .expect(1)
            .mount(&server)
            .await;

        adapter
            .start_session(
                "session-1",
                Path::new("/tmp/project"),
                Path::new("/tmp/project"),
                &memories,
            )
            .await
            .expect("session start should succeed");
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn secret_env_var_indirection_adds_bearer_auth() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _direct_secret_guard = EnvVarGuard::set("AGENTMEMORY_SECRET", "");
        let _indirect_secret_guard = EnvVarGuard::set("CODEX_AGENTMEMORY_SECRET", "indirect");
        let mut memories = test_memories(&server);
        memories.agentmemory.secret_env_var = "CODEX_AGENTMEMORY_SECRET".to_string();

        Mock::given(method("POST"))
            .and(path("/agentmemory/session/start"))
            .and(header("authorization", "Bearer indirect"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "context": "" })))
            .expect(1)
            .mount(&server)
            .await;

        adapter
            .start_session(
                "session-1",
                Path::new("/tmp/project"),
                Path::new("/tmp/project"),
                &memories,
            )
            .await
            .expect("session start should succeed");
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn agentmemory_url_env_override_beats_config_base_url() {
        let env_server = MockServer::start().await;
        let config_server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _url_guard = EnvVarGuard::set("AGENTMEMORY_URL", env_server.uri().as_str());
        let mut memories = test_memories(&config_server);
        memories.agentmemory.base_url = config_server.uri();

        Mock::given(method("POST"))
            .and(path("/agentmemory/session/start"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "context": "" })))
            .expect(1)
            .mount(&env_server)
            .await;

        adapter
            .start_session(
                "session-1",
                Path::new("/tmp/project"),
                Path::new("/tmp/project"),
                &memories,
            )
            .await
            .expect("session start should succeed");

        assert!(
            config_server
                .received_requests()
                .await
                .unwrap_or_default()
                .is_empty(),
            "config base_url should be ignored when AGENTMEMORY_URL is set",
        );
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn enrich_context_sends_claude_parity_payload_for_supported_tool_names() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _url_guard = EnvVarGuard::set("AGENTMEMORY_URL", "");
        let memories = test_memories(&server);

        Mock::given(method("POST"))
            .and(path("/agentmemory/enrich"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "context": "" })))
            .expect(5)
            .mount(&server)
            .await;

        for (tool_name, input) in [
            ("Edit", json!({ "paths": ["src/lib.rs"] })),
            ("Write", json!({ "file_path": "src/new.rs" })),
            ("Read", json!({ "path": "src/main.rs" })),
            ("Glob", json!({ "dir_path": "/tmp/project" })),
            (
                "Grep",
                json!({ "paths": ["src"], "pattern": "memory_recall" }),
            ),
        ] {
            adapter
                .enrich_context("session-1", tool_name, &input, &memories)
                .await
                .expect("enrichment should succeed");
        }

        let request_summaries = server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|request| {
                serde_json::from_slice::<serde_json::Value>(&request.body)
                    .expect("agentmemory enrich body should be valid json")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            request_summaries,
            vec![
                json!({
                    "sessionId": "session-1",
                    "files": ["src/lib.rs"],
                    "terms": [],
                    "toolName": "Edit",
                }),
                json!({
                    "sessionId": "session-1",
                    "files": ["src/new.rs"],
                    "terms": [],
                    "toolName": "Write",
                }),
                json!({
                    "sessionId": "session-1",
                    "files": ["src/main.rs"],
                    "terms": [],
                    "toolName": "Read",
                }),
                json!({
                    "sessionId": "session-1",
                    "files": ["/tmp/project"],
                    "terms": [],
                    "toolName": "Glob",
                }),
                json!({
                    "sessionId": "session-1",
                    "files": ["src"],
                    "terms": ["memory_recall"],
                    "toolName": "Grep",
                }),
            ]
        );
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn refresh_context_posts_query_aware_payload() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _url_guard = EnvVarGuard::set("AGENTMEMORY_URL", "");
        let memories = test_memories(&server);

        Mock::given(method("POST"))
            .and(path("/agentmemory/context/refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "context": "<agentmemory-context>fresh</agentmemory-context>",
                "skipped": false,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let result = adapter
            .refresh_context_result(
                "session-1",
                Path::new("/tmp/project"),
                "debug agentmemory refresh semantics",
                &memories,
            )
            .await
            .expect("refresh context should succeed");

        assert_eq!(
            (result.context, result.skipped),
            (
                "<agentmemory-context>fresh</agentmemory-context>".to_string(),
                false
            )
        );

        let requests = server.received_requests().await.unwrap_or_default();
        let body = serde_json::from_slice::<serde_json::Value>(&requests[0].body)
            .expect("refresh request body should be json");
        assert_eq!(
            body,
            json!({
                "sessionId": "session-1",
                "project": "/tmp/project",
                "query": "debug agentmemory refresh semantics",
            })
        );
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn refresh_context_result_preserves_retrieval_trace_summary() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _url_guard = EnvVarGuard::set("AGENTMEMORY_URL", "");
        let memories = test_memories(&server);

        Mock::given(method("POST"))
            .and(path("/agentmemory/context/refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "context": "<agentmemory-context>fresh</agentmemory-context>",
                "skipped": false,
                "trace": {
                    "queryTerms": ["debug", "agentmemory", "refresh"],
                    "laneBudgets": { "hot": 100, "warm": 200, "cold": 300 },
                    "laneUsage": { "hot": 80, "warm": 140 },
                    "selected": [
                        {
                            "id": "capsule:turn-1",
                            "lane": "hot",
                            "decision": "selected_lane_budget",
                            "preview": "recent turn capsule"
                        }
                    ],
                    "skipped": [
                        {
                            "id": "memory:old",
                            "lane": "cold",
                            "decision": "skipped_total_budget",
                            "preview": "older memory"
                        }
                    ]
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let result = adapter
            .refresh_context_result(
                "session-1",
                Path::new("/tmp/project"),
                "debug agentmemory refresh semantics",
                &memories,
            )
            .await
            .expect("refresh context should succeed");

        let trace = result
            .retrieval_trace_summary()
            .expect("retrieval trace summary should exist");
        assert_eq!(
            trace,
            AgentmemoryRetrievalTraceSummary {
                query_terms: vec![
                    "debug".to_string(),
                    "agentmemory".to_string(),
                    "refresh".to_string(),
                ],
                selected_count: 1,
                skipped_count: 1,
                lane_budgets: [
                    ("cold".to_string(), 300_usize),
                    ("hot".to_string(), 100_usize),
                    ("warm".to_string(), 200_usize),
                ]
                .into_iter()
                .collect(),
                lane_usage: [
                    ("hot".to_string(), 80_usize),
                    ("warm".to_string(), 140_usize),
                ]
                .into_iter()
                .collect(),
                selected: vec![crate::agentmemory::retrieval_trace::AgentmemoryRetrievalTraceCandidateSummary {
                    id: "capsule:turn-1".to_string(),
                    lane: "hot".to_string(),
                    decision: "selected_lane_budget".to_string(),
                    preview: "recent turn capsule".to_string(),
                }],
                skipped: vec![crate::agentmemory::retrieval_trace::AgentmemoryRetrievalTraceCandidateSummary {
                    id: "memory:old".to_string(),
                    lane: "cold".to_string(),
                    decision: "skipped_total_budget".to_string(),
                    preview: "older memory".to_string(),
                }],
            }
        );
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn recall_context_posts_query_aware_payload() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _url_guard = EnvVarGuard::set("AGENTMEMORY_URL", "");
        let memories = test_memories(&server);

        Mock::given(method("POST"))
            .and(path("/agentmemory/context"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "context": "<agentmemory-context>recall</agentmemory-context>",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let context = adapter
            .recall_context(
                "session-1",
                Path::new("/tmp/project"),
                Some("debug agentmemory recall semantics"),
                DEFAULT_RUNTIME_RECALL_TOKEN_BUDGET,
                &memories,
            )
            .await
            .expect("recall context should succeed");

        assert_eq!(context, "<agentmemory-context>recall</agentmemory-context>");

        let requests = server.received_requests().await.unwrap_or_default();
        let body = serde_json::from_slice::<serde_json::Value>(&requests[0].body)
            .expect("recall request body should be json");
        assert_eq!(
            body,
            json!({
                "sessionId": "session-1",
                "project": "/tmp/project",
                "budget": DEFAULT_RUNTIME_RECALL_TOKEN_BUDGET,
                "query": "debug agentmemory recall semantics",
            })
        );
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn remember_memory_posts_content_and_returns_json() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _url_guard = EnvVarGuard::set("AGENTMEMORY_URL", "");
        let memories = test_memories(&server);

        Mock::given(method("POST"))
            .and(path("/agentmemory/remember"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "success": true,
                "memory": {
                    "id": "mem-1",
                    "content": "remember this"
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let response = adapter
            .remember_memory("remember this", &memories)
            .await
            .expect("remember should succeed");

        assert_eq!(
            response,
            json!({
                "success": true,
                "memory": {
                    "id": "mem-1",
                    "content": "remember this"
                }
            })
        );
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn list_missions_sends_project_filters_and_returns_json() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _url_guard = EnvVarGuard::set("AGENTMEMORY_URL", "");
        let memories = test_memories(&server);

        Mock::given(method("GET"))
            .and(path("/agentmemory/missions"))
            .and(query_param("project", "/tmp/project"))
            .and(query_param("status", "blocked"))
            .and(query_param("owner", "agent-1"))
            .and(query_param("limit", "5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "missions": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let response = adapter
            .list_missions(
                Path::new("/tmp/project"),
                Some("blocked"),
                Some("agent-1"),
                Some(5),
                &memories,
            )
            .await
            .expect("list missions should succeed");

        assert_eq!(
            response,
            json!({
                "success": true,
                "missions": []
            })
        );
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn get_mission_fetches_by_id() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _url_guard = EnvVarGuard::set("AGENTMEMORY_URL", "");
        let memories = test_memories(&server);

        Mock::given(method("GET"))
            .and(path("/agentmemory/missions/msn_123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "mission": { "id": "msn_123" }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let response = adapter
            .get_mission("msn_123", &memories)
            .await
            .expect("get mission should succeed");

        assert_eq!(
            response,
            json!({
                "success": true,
                "mission": { "id": "msn_123" }
            })
        );
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn list_handoffs_sends_scope_filters_and_returns_json() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _url_guard = EnvVarGuard::set("AGENTMEMORY_URL", "");
        let memories = test_memories(&server);

        Mock::given(method("GET"))
            .and(path("/agentmemory/handoffs"))
            .and(query_param("project", "/tmp/project"))
            .and(query_param("scopeType", "mission"))
            .and(query_param("scopeId", "msn_123"))
            .and(query_param("limit", "3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "handoffPackets": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let response = adapter
            .list_handoffs(
                Path::new("/tmp/project"),
                Some("mission"),
                Some("msn_123"),
                Some(3),
                &memories,
            )
            .await
            .expect("list handoffs should succeed");

        assert_eq!(
            response,
            json!({
                "success": true,
                "handoffPackets": []
            })
        );
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn get_handoff_fetches_by_id() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _url_guard = EnvVarGuard::set("AGENTMEMORY_URL", "");
        let memories = test_memories(&server);

        Mock::given(method("GET"))
            .and(path("/agentmemory/handoffs/hdf_123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "handoffPacket": { "id": "hdf_123" }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let response = adapter
            .get_handoff("hdf_123", &memories)
            .await
            .expect("get handoff should succeed");

        assert_eq!(
            response,
            json!({
                "success": true,
                "handoffPacket": { "id": "hdf_123" }
            })
        );
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn generate_handoff_posts_scope_payload_and_returns_json() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _url_guard = EnvVarGuard::set("AGENTMEMORY_URL", "");
        let memories = test_memories(&server);

        Mock::given(method("POST"))
            .and(path("/agentmemory/handoffs/generate"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "success": true,
                "handoffPacket": { "id": "hdf_456" }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let response = adapter
            .generate_handoff("mission", "msn_123", Path::new("/tmp/project"), &memories)
            .await
            .expect("generate handoff should succeed");

        assert_eq!(
            response,
            json!({
                "success": true,
                "handoffPacket": { "id": "hdf_456" }
            })
        );

        let requests = server.received_requests().await.unwrap_or_default();
        let body = serde_json::from_slice::<serde_json::Value>(&requests[0].body)
            .expect("generate handoff request body should be json");
        assert_eq!(
            body,
            json!({
                "scopeType": "mission",
                "scopeId": "msn_123",
                "project": "/tmp/project",
            })
        );
    }
}
