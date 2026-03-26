//! Agentmemory integration adapter.
//!
//! This module provides the seam for integrating the `agentmemory` service
//! as a replacement for Codex's native memory engine.

use std::path::Path;
use std::sync::OnceLock;
use serde_json::json;

/// A placeholder adapter struct for agentmemory integration.
#[derive(Debug, Default, Clone)]
pub struct AgentmemoryAdapter {
    // Configuration and state will be added here in subsequent PRs.
}

/// A shared, pooled HTTP client for agentmemory interactions.
/// Reusing the client allows connection pooling (keep-alive) for high throughput.
static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn get_client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder().build().unwrap_or_default()
    })
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
        
        let mut instructions = "Use the `AgentMemory` tools to search and retrieve relevant memory.\n\
             Your context is bounded; use targeted queries to expand details as needed.".to_string();

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

    /// Transforms Codex's internal hook payloads into Claude-parity structures 
    /// expected by the `agentmemory` REST API. This provides a central, malleable
    /// place to adjust mapping logic in the future without touching the hooks engine.
    fn format_claude_parity_payload(&self, event_name: &str, payload: serde_json::Value) -> serde_json::Value {
        let session_id = payload.get("session_id").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        
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
                event_name, url, e
            );
        }
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
        let res = client.post(&url).json(&json!({"all": true})).send().await.map_err(|e| e.to_string())?;
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
