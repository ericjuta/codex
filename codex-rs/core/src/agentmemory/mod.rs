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

        if let Ok(res) = context_result {
            if let Ok(json_res) = res.json::<serde_json::Value>().await {
                if let Some(context_str) = json_res.get("context").and_then(|v| v.as_str()) {
                    if !context_str.is_empty() {
                        instructions.push_str("\n\n");
                        instructions.push_str(context_str);
                    }
                }
            }
        }

        Some(instructions)
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

        let tool_input = payload_map
            .get("command")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let tool_output = payload_map
            .get("tool_response")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let error = payload_map
            .get("tool_response")
            .and_then(|value| value.get("error"))
            .cloned()
            .unwrap_or_else(|| tool_output.clone());

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
            "PreToolUse" => (
                "pre_tool_use",
                json!({
                    "session_id": session_id,
                    "turn_id": payload_map.get("turn_id").cloned().unwrap_or(serde_json::Value::Null),
                    "cwd": cwd,
                    "model": payload_map.get("model").cloned().unwrap_or(serde_json::Value::Null),
                    "permission_mode": payload_map.get("permission_mode").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_name": payload_map.get("tool_name").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_use_id": payload_map.get("tool_use_id").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_input": tool_input,
                }),
            ),
            "PostToolUse" => (
                "post_tool_use",
                json!({
                    "session_id": session_id,
                    "turn_id": payload_map.get("turn_id").cloned().unwrap_or(serde_json::Value::Null),
                    "cwd": cwd,
                    "model": payload_map.get("model").cloned().unwrap_or(serde_json::Value::Null),
                    "permission_mode": payload_map.get("permission_mode").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_name": payload_map.get("tool_name").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_use_id": payload_map.get("tool_use_id").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_input": tool_input,
                    "tool_output": tool_output,
                }),
            ),
            "PostToolUseFailure" => (
                "post_tool_failure",
                json!({
                    "session_id": session_id,
                    "turn_id": payload_map.get("turn_id").cloned().unwrap_or(serde_json::Value::Null),
                    "cwd": cwd,
                    "model": payload_map.get("model").cloned().unwrap_or(serde_json::Value::Null),
                    "permission_mode": payload_map.get("permission_mode").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_name": payload_map.get("tool_name").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_use_id": payload_map.get("tool_use_id").cloned().unwrap_or(serde_json::Value::Null),
                    "tool_input": tool_input,
                    "error": error,
                }),
            ),
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
