//! Agentmemory integration adapter.
//!
//! This module provides the seam for integrating the `agentmemory` service
//! as a replacement for Codex's native memory engine.

use crate::config::types::AgentmemoryConfig;
use crate::config::types::MemoriesConfig;
use codex_git_utils::get_git_repo_root;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

/// A placeholder adapter struct for agentmemory integration.
#[derive(Debug, Default, Clone)]
pub struct AgentmemoryAdapter {
    // Configuration and state will be added here in subsequent PRs.
}

/// A shared, pooled HTTP client for agentmemory interactions.
/// Reusing the client allows connection pooling (keep-alive) for high throughput.
static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

pub(crate) const DEFAULT_RUNTIME_RECALL_TOKEN_BUDGET: usize = 2_000;
const MEMORY_RECALL_DEVELOPER_INSTRUCTIONS: &str = "Use the `memory_recall` tool when the user asks about prior work, earlier decisions, previous failures, resumed threads, or other historical context that is not fully present in the current thread.\n\
     Agentmemory startup context may be attached below when available.\n\
     Prefer targeted recall queries naming the feature, file, bug, or decision you need.\n\
     If the current runtime exposes tools through a wrapper surface (for example, `exec` with nested `tools`), treat the callable nested tool surface as authoritative when checking whether `memory_recall` is available.\n\
     Do not call `memory_recall` on every turn; first use the current thread context, then recall memory when that context appears insufficient.";

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
struct AgentmemoryContextResult {
    #[serde(default)]
    context: String,
}

fn get_client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| reqwest::Client::builder().build().unwrap_or_default())
}

fn extract_project_and_cwd(payload: &serde_json::Value) -> (String, String) {
    let cwd = payload
        .get("cwd")
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    let project = if cwd.is_empty() {
        String::new()
    } else {
        get_git_repo_root(Path::new(&cwd))
            .unwrap_or_else(|| Path::new(&cwd).to_path_buf())
            .to_string_lossy()
            .into_owned()
    };
    (project, cwd)
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
            .and_then(|raw| match raw.trim().to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            })
            .unwrap_or(memories.agentmemory.inject_context)
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

    fn request_builder(&self, url: &str, memories: &MemoriesConfig) -> reqwest::RequestBuilder {
        let builder = get_client().post(url);
        if let Some(secret) = self.auth_secret(memories) {
            builder.bearer_auth(secret)
        } else {
            builder
        }
    }

    async fn parse_context_result(response: reqwest::Response) -> Result<String, String> {
        let payload = response
            .json::<AgentmemoryContextResult>()
            .await
            .map_err(|err| err.to_string())?;
        Ok(payload.context)
    }

    /// Builds the developer instructions for the assistant-facing memory recall
    /// tool when the `agentmemory` backend is active.
    pub async fn build_startup_developer_instructions(
        &self,
        _codex_home: &Path,
        _token_budget: usize,
    ) -> Option<String> {
        Some(MEMORY_RECALL_DEVELOPER_INSTRUCTIONS.to_string())
    }

    /// Attempts to parse a tool command string as JSON to recover structured
    /// arguments. Falls back to the original string value on parse failure.
    fn parse_structured_tool_input(raw: &serde_json::Value) -> serde_json::Value {
        if let Some(s) = raw.as_str()
            && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s)
            && parsed.is_object()
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

    /// Transforms Codex's internal hook payloads into Claude-parity structures
    /// expected by the `agentmemory` REST API. This provides a central, malleable
    /// place to adjust mapping logic in the future without touching the hooks engine.
    fn format_claude_parity_payload(
        &self,
        event_name: &str,
        payload: serde_json::Value,
    ) -> serde_json::Value {
        let session_id = payload
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let (project, cwd) = extract_project_and_cwd(&payload);

        let timestamp = chrono::Utc::now().to_rfc3339();
        let hook_type = normalize_hook_type(event_name);

        json!({
            "sessionId": session_id,
            "hookType": hook_type,
            "project": project,
            "cwd": cwd,
            "timestamp": timestamp,
            "data": payload,
        })
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
        let body = self.format_claude_parity_payload(event_name, payload_json);

        match self
            .request_builder(&url, memories)
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
    pub async fn recall_context(
        &self,
        session_id: &str,
        project: &Path,
        query: Option<&str>,
        token_budget: usize,
        memories: &MemoriesConfig,
    ) -> Result<String, String> {
        let url = format!("{}/agentmemory/context", self.api_base(memories));

        let mut body = json!({
            "sessionId": session_id,
            "project": project.to_string_lossy(),
            "budget": token_budget,
        });
        if let Some(q) = query {
            body["query"] = serde_json::Value::String(q.to_string());
        }

        let res = self
            .request_builder(&url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err(format!(
                "Context retrieval failed with status {}",
                res.status()
            ));
        }

        let json_res: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
        Ok(json_res
            .get("context")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string())
    }

    pub(crate) async fn recall_for_runtime(
        &self,
        session_id: &str,
        project: &Path,
        query: Option<&str>,
        memories: &MemoriesConfig,
    ) -> Result<MemoryRecallResult, String> {
        let context = self
            .recall_context(
                session_id,
                project,
                query,
                DEFAULT_RUNTIME_RECALL_TOKEN_BUDGET,
                memories,
            )
            .await?;

        Ok(MemoryRecallResult {
            recalled: !context.trim().is_empty(),
            context,
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
            .request_builder(&url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("Session start failed with status {}", res.status()));
        }
        Self::parse_context_result(res).await
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
            .request_builder(&url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if !res.status().is_success() {
            return Err(format!(
                "Context enrichment failed with status {}",
                res.status()
            ));
        }
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
            .request_builder(&url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("Session end failed with status {}", res.status()));
        }
        Ok(())
    }

    /// Asynchronously triggers a memory refresh/update operation in `agentmemory`.
    pub(crate) async fn update_memories(
        &self,
        memories: &MemoriesConfig,
    ) -> Result<AgentmemoryConsolidateResult, String> {
        let url = format!("{}/agentmemory/consolidate", self.api_base(memories));
        let res = self
            .request_builder(&url, memories)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("Consolidate failed with status {}", res.status()));
        }
        res.json::<AgentmemoryConsolidateResult>()
            .await
            .map_err(|e| e.to_string())
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
            .request_builder(&url, memories)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("Summarize failed with status {}", res.status()));
        }
        res.json::<AgentmemorySummarizeResult>()
            .await
            .map_err(|e| e.to_string())
    }

    /// Asynchronously drops/clears the memory store in `agentmemory`.
    pub async fn drop_memories(&self, memories: &MemoriesConfig) -> Result<(), String> {
        let url = format!("{}/agentmemory/forget", self.api_base(memories));
        let res = self
            .request_builder(&url, memories)
            .json(&json!({"all": true}))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("Forget failed with status {}", res.status()));
        }
        Ok(())
    }
}
fn normalize_hook_type(event_name: &str) -> &str {
    match event_name {
        "SessionStart" => "session_start",
        "UserPromptSubmit" => "prompt_submit",
        "PreToolUse" => "pre_tool_use",
        "PostToolUse" => "post_tool_use",
        "PostToolUseFailure" => "post_tool_failure",
        "AssistantResult" => "assistant_result",
        "SubagentStart" => "subagent_start",
        "SubagentStop" => "subagent_stop",
        "Stop" => "stop",
        "Notification" => "notification",
        "TaskCompleted" => "task_completed",
        "SessionEnd" => "session_end",
        _ => event_name,
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

        assert!(instructions.contains("Use the `memory_recall` tool"));
        assert!(instructions.contains("prior work, earlier decisions, previous failures"));
        assert!(instructions.contains("Agentmemory startup context may be attached below"));
        assert!(instructions.contains(
            "Prefer targeted recall queries naming the feature, file, bug, or decision you need"
        ));
        assert!(instructions.contains("Do not call `memory_recall` on every turn"));
    }

    #[test]
    fn format_claude_parity_payload_normalizes_codex_hook_names() {
        let adapter = AgentmemoryAdapter::new();
        let payload = json!({ "session_id": "session-123" });

        let prompt_submit =
            adapter.format_claude_parity_payload("UserPromptSubmit", payload.clone());
        assert_eq!(prompt_submit["hookType"], json!("prompt_submit"));
        assert_eq!(prompt_submit["sessionId"], json!("session-123"));

        let post_tool_failure =
            adapter.format_claude_parity_payload("PostToolUseFailure", payload.clone());
        assert_eq!(post_tool_failure["hookType"], json!("post_tool_failure"));

        let stop = adapter.format_claude_parity_payload("Stop", payload);
        assert_eq!(stop["hookType"], json!("stop"));

        let session_end = adapter
            .format_claude_parity_payload("SessionEnd", json!({ "session_id": "session-1" }));
        assert_eq!(session_end["hookType"], json!("session_end"));
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

    #[test]
    fn test_format_claude_parity_payload() {
        let adapter = AgentmemoryAdapter::new();
        let raw_payload = json!({
            "session_id": "1234",
            "turn_id": "turn-5",
            "cwd": "/tmp/project",
            "command": "echo hello"
        });

        let formatted = adapter.format_claude_parity_payload("PreToolUse", raw_payload.clone());

        assert_eq!(formatted["sessionId"], "1234");
        assert_eq!(formatted["hookType"], "pre_tool_use");
        assert_eq!(formatted["project"], "/tmp/project");
        assert_eq!(formatted["cwd"], "/tmp/project");
        assert!(formatted.get("timestamp").is_some());
        assert_eq!(formatted["data"], raw_payload);
    }
}
