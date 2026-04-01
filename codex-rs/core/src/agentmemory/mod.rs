//! Agentmemory integration adapter.
//!
//! This module provides the seam for integrating the `agentmemory` service
//! as a replacement for Codex's native memory engine.

use serde::Serialize;
use serde_json::json;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryRecallResult {
    pub(crate) recalled: bool,
    pub(crate) context: String,
}

fn get_client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| reqwest::Client::builder().build().unwrap_or_default())
}

impl AgentmemoryAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    fn api_base(&self) -> String {
        std::env::var("III_REST_PORT")
            .map(|p| format!("http://localhost:{}", p))
            .unwrap_or_else(|_| "http://localhost:3111".to_string())
    }

    /// Builds the developer instructions for startup memory injection
    /// using the `agentmemory` retrieval stack.
    ///
    /// This retrieves context bounded by a token budget and explicitly
    /// uses hybrid search semantics rather than loading large static artifacts.
    pub async fn build_startup_developer_instructions(
        &self,
        codex_home: &Path,
        token_budget: usize,
    ) -> Option<String> {
        let client = get_client();
        let url = format!("{}/agentmemory/context", self.api_base());
        let project = std::env::current_dir()
            .unwrap_or_else(|_| codex_home.to_path_buf())
            .to_string_lossy()
            .into_owned();

        let request_body = json!({
            "sessionId": "startup", // We don't have a session ID at this exact moment easily accessible, but "startup" excludes it safely.
            "project": project,
            "budget": token_budget
        });

        let context_result = client.post(&url).json(&request_body).send().await;

        let mut instructions =
            "Use the `memory_recall` tool when the user asks about prior work, earlier decisions, previous failures, resumed threads, or other historical context that is not fully present in the current thread.\n\
             Agentmemory startup context may be attached below when available.\n\
             Prefer targeted recall queries naming the feature, file, bug, or decision you need.\n\
             Do not call `memory_recall` on every turn; first use the current thread context, then recall memory when that context appears insufficient."
                .to_string();

        if let Ok(res) = context_result
            && let Ok(json_res) = res.json::<serde_json::Value>().await
            && let Some(context_str) = json_res.get("context").and_then(|v| v.as_str())
            && !context_str.is_empty()
        {
            instructions.push_str("\n\n");
            instructions.push_str(context_str);
        }

        Some(instructions)
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

        if let Some(obj) = tool_input.as_object() {
            // File path fields
            for key in &["file_path", "path", "dir_path"] {
                if let Some(v) = obj.get(*key).and_then(|v| v.as_str())
                    && !v.is_empty()
                {
                    files.push(v.to_string());
                }
            }
            // Array of paths
            if let Some(arr) = obj.get("paths").and_then(|v| v.as_array()) {
                for item in arr {
                    if let Some(s) = item.as_str()
                        && !s.is_empty()
                    {
                        files.push(s.to_string());
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

        let timestamp = chrono::Utc::now().to_rfc3339();

        json!({
            "sessionId": session_id,
            "hookType": event_name,
            "timestamp": timestamp,
            "data": payload,
        })
    }

    /// Asynchronously captures and stores lifecycle events in `agentmemory`.
    ///
    /// This method allows Codex hooks (like `SessionStart`, `PostToolUse`) to
    /// be transmitted without blocking the hot path of the shell or model output.
    pub async fn capture_event(&self, event_name: &str, payload_json: serde_json::Value) {
        let url = format!("{}/agentmemory/observe", self.api_base());
        let client = get_client();

        let body = self.format_claude_parity_payload(event_name, payload_json);

        if let Err(e) = client.post(&url).json(&body).send().await {
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
    ) -> Result<String, String> {
        let client = get_client();
        let url = format!("{}/agentmemory/context", self.api_base());

        let mut body = json!({
            "sessionId": session_id,
            "project": project.to_string_lossy(),
            "budget": token_budget,
        });
        if let Some(q) = query {
            body["query"] = serde_json::Value::String(q.to_string());
        }

        let res = client
            .post(&url)
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
    ) -> Result<MemoryRecallResult, String> {
        let context = self
            .recall_context(
                session_id,
                project,
                query,
                DEFAULT_RUNTIME_RECALL_TOKEN_BUDGET,
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
    ) -> Result<(), String> {
        let url = format!("{}/agentmemory/session/start", self.api_base());
        let client = get_client();
        let body = json!({
            "sessionId": session_id,
            "project": project.display().to_string(),
            "cwd": cwd.display().to_string(),
        });
        let res = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("Session start failed with status {}", res.status()));
        }
        Ok(())
    }

    /// Marks a session completed so Agentmemory's viewer can stop showing it as active.
    pub async fn end_session(&self, session_id: &str) -> Result<(), String> {
        let url = format!("{}/agentmemory/session/end", self.api_base());
        let client = get_client();
        let body = json!({ "sessionId": session_id });
        let res = client
            .post(&url)
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
    pub async fn update_memories(&self) -> Result<(), String> {
        let url = format!("{}/agentmemory/consolidate", self.api_base());
        let client = get_client();
        let res = client.post(&url).send().await.map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("Consolidate failed with status {}", res.status()));
        }
        Ok(())
    }

    /// Asynchronously drops/clears the memory store in `agentmemory`.
    pub async fn drop_memories(&self) -> Result<(), String> {
        let url = format!("{}/agentmemory/forget", self.api_base());
        let client = get_client();
        let res = client
            .post(&url)
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
#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::ffi::OsString;
    use std::sync::Mutex;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_json;
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

        fn unset(key: &'static str) -> Self {
            let original = std::env::var_os(key);
            unsafe {
                std::env::remove_var(key);
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

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn test_startup_instructions_describe_current_runtime_surface() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _agentmemory_url_guard = EnvVarGuard::set("AGENTMEMORY_URL", server.uri().as_str());

        Mock::given(method("POST"))
            .and(path("/agentmemory/context"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "context": ""
            })))
            .expect(1)
            .mount(&server)
            .await;

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

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn test_startup_instructions_append_retrieved_context() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _guard = ENV_LOCK.lock().expect("lock env");
        let _agentmemory_url_guard = EnvVarGuard::set("AGENTMEMORY_URL", server.uri().as_str());

        Mock::given(method("POST"))
            .and(path("/agentmemory/context"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "context": "<agentmemory-context>remember this</agentmemory-context>"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let instructions = adapter
            .build_startup_developer_instructions(Path::new("/tmp/project"), 256)
            .await
            .expect("instructions should be returned");

        assert!(instructions.contains("<agentmemory-context>remember this</agentmemory-context>"));
    }

    #[test]
    fn test_format_claude_parity_payload() {
        let adapter = AgentmemoryAdapter::new();
        let raw_payload = json!({
            "session_id": "1234",
            "turn_id": "turn-5",
            "command": "echo hello"
        });

        let formatted = adapter.format_claude_parity_payload("PreToolUse", raw_payload.clone());

        assert_eq!(formatted["sessionId"], "1234");
        assert_eq!(formatted["hookType"], "PreToolUse");
        assert!(formatted.get("timestamp").is_some());
        assert_eq!(formatted["data"], raw_payload);
    }
}
