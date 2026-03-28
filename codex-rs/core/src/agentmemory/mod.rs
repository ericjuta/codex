//! Agentmemory integration adapter.
//!
//! This module provides the seam for integrating the `agentmemory` service
//! as a replacement for Codex's native memory engine.

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

fn get_client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| reqwest::Client::builder().build().unwrap_or_default())
}

impl AgentmemoryAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    fn api_base(&self) -> String {
        if let Some(url) = std::env::var("AGENTMEMORY_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
        {
            return url;
        }

        std::env::var("III_REST_PORT")
            .map(|port| format!("http://127.0.0.1:{port}"))
            .unwrap_or_else(|_| "http://127.0.0.1:3111".to_string())
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
            "Use the `AgentMemory` tools to search and retrieve relevant memory.\n\
             Your context is bounded; use targeted queries to expand details as needed."
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
            // Search / pattern fields
            for key in &["query", "pattern", "glob"] {
                if let Some(v) = obj.get(*key).and_then(|v| v.as_str())
                    && !v.is_empty()
                {
                    search_terms.push(v.to_string());
                }
            }
        }

        (files, search_terms)
    }

    /// Maximum length for assistant text stored in observations.
    const ASSISTANT_TEXT_MAX_BYTES: usize = 4096;

    /// Truncates text to a safe size for observation storage.
    fn truncate_text(text: &str, max_bytes: usize) -> &str {
        if text.len() <= max_bytes {
            return text;
        }
        // Find a char boundary at or before max_bytes
        let mut end = max_bytes;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        &text[..end]
    }

    /// Transforms Codex hook payloads into the canonical Agentmemory hook schema.
    fn format_agentmemory_payload(
        &self,
        event_name: &str,
        payload: serde_json::Value,
    ) -> serde_json::Value {
        let payload_map = payload.as_object().cloned().unwrap_or_default();
        let session_id = payload_map
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let cwd = payload_map
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| ".".to_string());
        let timestamp = chrono::Utc::now().to_rfc3339();

        // Parse structured tool input from the command string when possible.
        let raw_tool_input = payload_map
            .get("command")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let tool_input = Self::parse_structured_tool_input(&raw_tool_input);

        let tool_output = payload_map
            .get("tool_response")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let error = payload_map
            .get("tool_response")
            .and_then(|value| value.get("error"))
            .cloned()
            .unwrap_or_else(|| tool_output.clone());

        // Extract file paths and search terms for enrichment.
        let (files, search_terms) = Self::extract_file_enrichment(&tool_input);

        let (hook_type, data) = match event_name {
            "SessionStart" => (
                "session_start",
                json!({
                    "session_id": session_id,
                    "cwd": cwd,
                    "model": payload_map.get("model").cloned().unwrap_or(serde_json::Value::Null),
                    "permission_mode": payload_map.get("permission_mode").cloned().unwrap_or(serde_json::Value::Null),
                    "transcript_path": payload_map.get("transcript_path").cloned().unwrap_or(serde_json::Value::Null),
                    "source": payload_map.get("source").cloned().unwrap_or(serde_json::Value::Null),
                }),
            ),
            "UserPromptSubmit" => (
                "prompt_submit",
                json!({
                    "session_id": session_id,
                    "turn_id": payload_map.get("turn_id").cloned().unwrap_or(serde_json::Value::Null),
                    "cwd": cwd,
                    "model": payload_map.get("model").cloned().unwrap_or(serde_json::Value::Null),
                    "permission_mode": payload_map.get("permission_mode").cloned().unwrap_or(serde_json::Value::Null),
                    "prompt": payload_map.get("prompt").cloned().unwrap_or(serde_json::Value::Null),
                }),
            ),
            "PreToolUse" => {
                let mut data = json!({
                    "session_id": session_id,
                    "turn_id": payload_map.get("turn_id").cloned().unwrap_or(serde_json::Value::Null),
                    "cwd": cwd,
                    "model": payload_map.get("model").cloned().unwrap_or(serde_json::Value::Null),
                    "permission_mode": payload_map.get("permission_mode").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_name": payload_map.get("tool_name").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_use_id": payload_map.get("tool_use_id").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_input": tool_input,
                });
                if !files.is_empty() {
                    data["files"] = json!(files);
                }
                if !search_terms.is_empty() {
                    data["search_terms"] = json!(search_terms);
                }
                ("pre_tool_use", data)
            }
            "PostToolUse" => {
                let mut data = json!({
                    "session_id": session_id,
                    "turn_id": payload_map.get("turn_id").cloned().unwrap_or(serde_json::Value::Null),
                    "cwd": cwd,
                    "model": payload_map.get("model").cloned().unwrap_or(serde_json::Value::Null),
                    "permission_mode": payload_map.get("permission_mode").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_name": payload_map.get("tool_name").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_use_id": payload_map.get("tool_use_id").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_input": tool_input,
                    "tool_output": tool_output,
                });
                // File-aware enrichment: surface paths and search terms.
                if !files.is_empty() {
                    data["files"] = json!(files);
                }
                if !search_terms.is_empty() {
                    data["search_terms"] = json!(search_terms);
                }
                ("post_tool_use", data)
            }
            "PostToolUseFailure" => {
                let mut data = json!({
                    "session_id": session_id,
                    "turn_id": payload_map.get("turn_id").cloned().unwrap_or(serde_json::Value::Null),
                    "cwd": cwd,
                    "model": payload_map.get("model").cloned().unwrap_or(serde_json::Value::Null),
                    "permission_mode": payload_map.get("permission_mode").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_name": payload_map.get("tool_name").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_use_id": payload_map.get("tool_use_id").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_input": tool_input,
                    "error": error,
                });
                if !files.is_empty() {
                    data["files"] = json!(files);
                }
                if !search_terms.is_empty() {
                    data["search_terms"] = json!(search_terms);
                }
                ("post_tool_failure", data)
            }
            "Stop" => (
                "stop",
                json!({
                    "session_id": session_id,
                    "turn_id": payload_map.get("turn_id").cloned().unwrap_or(serde_json::Value::Null),
                    "cwd": cwd,
                    "model": payload_map.get("model").cloned().unwrap_or(serde_json::Value::Null),
                    "permission_mode": payload_map.get("permission_mode").cloned().unwrap_or(serde_json::Value::Null),
                    "last_assistant_message": payload_map.get("last_assistant_message").cloned().unwrap_or(serde_json::Value::Null),
                }),
            ),
            "AssistantResult" => {
                let assistant_text = payload_map
                    .get("assistant_text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let truncated = Self::truncate_text(assistant_text, Self::ASSISTANT_TEXT_MAX_BYTES);
                (
                    "assistant_result",
                    json!({
                        "session_id": session_id,
                        "turn_id": payload_map.get("turn_id").cloned().unwrap_or(serde_json::Value::Null),
                        "cwd": cwd,
                        "model": payload_map.get("model").cloned().unwrap_or(serde_json::Value::Null),
                        "assistant_text": truncated,
                        "is_final": payload_map.get("is_final").cloned().unwrap_or(json!(true)),
                    }),
                )
            }
            _ => (event_name, serde_json::Value::Object(payload_map.clone())),
        };

        json!({
            "sessionId": session_id,
            "hookType": hook_type,
            "project": cwd,
            "cwd": payload_map.get("cwd").cloned().unwrap_or_else(|| serde_json::Value::String(".".to_string())),
            "timestamp": timestamp,
            "data": data,
        })
    }

    /// Asynchronously captures and stores lifecycle events in `agentmemory`.
    ///
    /// This method allows Codex hooks (like `SessionStart`, `PostToolUse`) to
    /// be transmitted without blocking the hot path of the shell or model output.
    pub async fn capture_event(&self, event_name: &str, payload_json: serde_json::Value) {
        let url = format!("{}/agentmemory/observe", self.api_base());
        let client = get_client();

        let body = self.format_agentmemory_payload(event_name, payload_json);

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

    #[test]
    fn test_format_agentmemory_payload_maps_prompt_submit_shape() {
        let adapter = AgentmemoryAdapter::new();
        let raw_payload = json!({
            "session_id": "1234",
            "turn_id": "turn-5",
            "cwd": "/tmp/project",
            "prompt": "ship it"
        });

        let formatted = adapter.format_agentmemory_payload("UserPromptSubmit", raw_payload);

        assert_eq!(formatted["sessionId"], "1234");
        assert_eq!(formatted["hookType"], "prompt_submit");
        assert_eq!(formatted["project"], "/tmp/project");
        assert_eq!(formatted["cwd"], "/tmp/project");
        assert!(formatted.get("timestamp").is_some());
        assert_eq!(formatted["data"]["prompt"], "ship it");
        assert_eq!(formatted["data"]["turn_id"], "turn-5");
    }

    #[test]
    fn test_format_agentmemory_payload_maps_post_tool_use_shape() {
        let adapter = AgentmemoryAdapter::new();
        let raw_payload = json!({
            "session_id": "1234",
            "turn_id": "turn-5",
            "cwd": "/tmp/project",
            "tool_name": "shell_command",
            "tool_use_id": "tool-1",
            "command": "printf hi",
            "tool_response": { "output": "hi" }
        });

        let formatted = adapter.format_agentmemory_payload("PostToolUse", raw_payload);

        assert_eq!(formatted["sessionId"], "1234");
        assert_eq!(formatted["hookType"], "post_tool_use");
        assert_eq!(formatted["data"]["tool_name"], "shell_command");
        assert_eq!(formatted["data"]["tool_input"], "printf hi");
        assert_eq!(formatted["data"]["tool_output"]["output"], "hi");
    }

    #[test]
    fn test_pre_tool_use_includes_structured_args_and_enrichment() {
        let adapter = AgentmemoryAdapter::new();
        let raw_payload = json!({
            "session_id": "s1",
            "turn_id": "t1",
            "cwd": "/proj",
            "tool_name": "grep",
            "tool_use_id": "tu-0",
            "command": r#"{"path":"/proj/src","pattern":"fn main","glob":"*.rs"}"#,
        });

        let formatted = adapter.format_agentmemory_payload("PreToolUse", raw_payload);

        assert_eq!(formatted["hookType"], "pre_tool_use");
        assert_eq!(formatted["data"]["tool_input"]["path"], "/proj/src");
        assert_eq!(formatted["data"]["tool_input"]["pattern"], "fn main");
        assert_eq!(formatted["data"]["files"][0], "/proj/src");
        assert_eq!(formatted["data"]["search_terms"][0], "fn main");
        assert_eq!(formatted["data"]["search_terms"][1], "*.rs");
    }

    #[test]
    fn test_structured_tool_input_parsed_from_json_command() {
        let adapter = AgentmemoryAdapter::new();
        let raw_payload = json!({
            "session_id": "s1",
            "turn_id": "t1",
            "cwd": "/proj",
            "tool_name": "read_file",
            "tool_use_id": "tu-1",
            "command": r#"{"file_path":"/proj/src/main.rs","offset":1,"limit":50}"#,
            "tool_response": { "text": "fn main() {}" }
        });

        let formatted = adapter.format_agentmemory_payload("PostToolUse", raw_payload);

        // tool_input should be the parsed object, not the raw string.
        assert_eq!(
            formatted["data"]["tool_input"]["file_path"],
            "/proj/src/main.rs"
        );
        assert_eq!(formatted["data"]["tool_input"]["offset"], 1);
        // File enrichment should surface the path.
        assert_eq!(formatted["data"]["files"][0], "/proj/src/main.rs");
    }

    #[test]
    fn test_non_json_command_preserved_as_string() {
        let adapter = AgentmemoryAdapter::new();
        let raw_payload = json!({
            "session_id": "s1",
            "turn_id": "t1",
            "cwd": "/proj",
            "tool_name": "shell",
            "tool_use_id": "tu-2",
            "command": "ls -la /tmp",
            "tool_response": { "output": "total 0" }
        });

        let formatted = adapter.format_agentmemory_payload("PostToolUse", raw_payload);
        assert_eq!(formatted["data"]["tool_input"], "ls -la /tmp");
        // No file enrichment for plain commands.
        assert!(formatted["data"].get("files").is_none());
    }

    #[test]
    fn test_file_enrichment_extracts_paths_and_search_terms() {
        let adapter = AgentmemoryAdapter::new();
        let raw_payload = json!({
            "session_id": "s1",
            "turn_id": "t1",
            "cwd": "/proj",
            "tool_name": "grep",
            "tool_use_id": "tu-3",
            "command": r#"{"path":"/proj/src","pattern":"TODO","glob":"*.rs"}"#,
            "tool_response": { "matches": [] }
        });

        let formatted = adapter.format_agentmemory_payload("PostToolUse", raw_payload);

        assert_eq!(formatted["data"]["files"][0], "/proj/src");
        assert_eq!(formatted["data"]["search_terms"][0], "TODO");
        assert_eq!(formatted["data"]["search_terms"][1], "*.rs");
    }

    #[test]
    fn test_file_enrichment_on_failure_event() {
        let adapter = AgentmemoryAdapter::new();
        let raw_payload = json!({
            "session_id": "s1",
            "turn_id": "t1",
            "cwd": "/proj",
            "tool_name": "read_file",
            "tool_use_id": "tu-4",
            "command": r#"{"file_path":"/proj/missing.rs"}"#,
            "tool_response": { "error": "file not found" }
        });

        let formatted = adapter.format_agentmemory_payload("PostToolUseFailure", raw_payload);

        assert_eq!(formatted["hookType"], "post_tool_failure");
        assert_eq!(formatted["data"]["files"][0], "/proj/missing.rs");
        assert_eq!(formatted["data"]["error"], "file not found");
    }

    #[test]
    fn test_assistant_result_payload_shape() {
        let adapter = AgentmemoryAdapter::new();
        let raw_payload = json!({
            "session_id": "s1",
            "turn_id": "t1",
            "cwd": "/proj",
            "model": "claude-opus-4-6",
            "assistant_text": "The build succeeded with no warnings.",
            "is_final": true,
        });

        let formatted = adapter.format_agentmemory_payload("AssistantResult", raw_payload);

        assert_eq!(formatted["hookType"], "assistant_result");
        assert_eq!(formatted["sessionId"], "s1");
        assert_eq!(
            formatted["data"]["assistant_text"],
            "The build succeeded with no warnings."
        );
        assert_eq!(formatted["data"]["is_final"], true);
        assert_eq!(formatted["data"]["turn_id"], "t1");
        assert_eq!(formatted["data"]["model"], "claude-opus-4-6");
    }

    #[test]
    fn test_assistant_result_truncates_long_text() {
        let adapter = AgentmemoryAdapter::new();
        let long_text = "x".repeat(8000);
        let raw_payload = json!({
            "session_id": "s1",
            "turn_id": "t1",
            "cwd": "/proj",
            "model": "test",
            "assistant_text": long_text,
        });

        let formatted = adapter.format_agentmemory_payload("AssistantResult", raw_payload);
        let stored_text = formatted["data"]["assistant_text"].as_str().unwrap();
        assert!(stored_text.len() <= AgentmemoryAdapter::ASSISTANT_TEXT_MAX_BYTES);
    }

    #[test]
    fn test_paths_array_enrichment() {
        let adapter = AgentmemoryAdapter::new();
        let raw_payload = json!({
            "session_id": "s1",
            "turn_id": "t1",
            "cwd": "/proj",
            "tool_name": "multi_edit",
            "tool_use_id": "tu-5",
            "command": r#"{"paths":["/proj/a.rs","/proj/b.rs"]}"#,
            "tool_response": { "ok": true }
        });

        let formatted = adapter.format_agentmemory_payload("PostToolUse", raw_payload);

        let files = formatted["data"]["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0], "/proj/a.rs");
        assert_eq!(files[1], "/proj/b.rs");
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn test_start_session_posts_expected_payload() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _agentmemory_url_guard = EnvVarGuard::set("AGENTMEMORY_URL", server.uri().as_str());

        Mock::given(method("POST"))
            .and(path("/agentmemory/session/start"))
            .and(body_json(json!({
                "sessionId": "session-1",
                "project": "/tmp/project",
                "cwd": "/tmp/project",
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        adapter
            .start_session(
                "session-1",
                Path::new("/tmp/project"),
                Path::new("/tmp/project"),
            )
            .await
            .expect("session start should succeed");
    }

    #[tokio::test]
    #[serial_test::serial(agentmemory_env)]
    async fn test_end_session_posts_expected_payload() {
        let server = MockServer::start().await;
        let adapter = AgentmemoryAdapter::new();
        let _agentmemory_url_guard = EnvVarGuard::set("AGENTMEMORY_URL", server.uri().as_str());

        Mock::given(method("POST"))
            .and(path("/agentmemory/session/end"))
            .and(body_json(json!({
                "sessionId": "session-1",
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        adapter
            .end_session("session-1")
            .await
            .expect("session end should succeed");
    }

    #[test]
    fn test_api_base_prefers_explicit_agentmemory_url() {
        let _guard = ENV_LOCK.lock().expect("lock env");
        let adapter = AgentmemoryAdapter::new();
        let _agentmemory_url_guard = EnvVarGuard::set("AGENTMEMORY_URL", "http://127.0.0.1:9999");
        let _rest_port_guard = EnvVarGuard::set("III_REST_PORT", "3111");

        assert_eq!(adapter.api_base(), "http://127.0.0.1:9999");
    }

    #[test]
    fn test_api_base_defaults_to_ipv4_loopback() {
        let _guard = ENV_LOCK.lock().expect("lock env");
        let adapter = AgentmemoryAdapter::new();
        let _agentmemory_url_guard = EnvVarGuard::unset("AGENTMEMORY_URL");
        let _rest_port_guard = EnvVarGuard::set("III_REST_PORT", "4242");

        assert_eq!(adapter.api_base(), "http://127.0.0.1:4242");
    }
}
