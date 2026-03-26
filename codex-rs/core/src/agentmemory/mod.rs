//! Agentmemory integration adapter.
//!
//! This module provides the seam for integrating the `agentmemory` service
//! as a replacement for Codex's native memory engine.

use std::path::Path;

/// A placeholder adapter struct for agentmemory integration.
#[derive(Debug, Default, Clone)]
pub struct AgentmemoryAdapter {
    // Configuration and state will be added here in subsequent PRs.
}

impl AgentmemoryAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the developer instructions for startup memory injection
    /// using the `agentmemory` retrieval stack.
    ///
    /// This retrieves context bounded by a token budget and explicitly
    /// uses hybrid search semantics rather than loading large static artifacts.
    pub async fn build_startup_developer_instructions(
        &self,
        _codex_home: &Path,
        _token_budget: usize,
    ) -> Option<String> {
        // TODO: Call agentmemory REST/MCP endpoints to fetch top-K results
        // For now, return a placeholder instructions block.
        Some(
            "Use the `AgentMemory` tools to search and retrieve relevant memory.\n\
             Your context is bounded; use targeted queries to expand details as needed."
                .to_string(),
        )
    }

    /// Asynchronously captures and stores lifecycle events in `agentmemory`.
    ///
    /// This method allows Codex hooks (like `SessionStart`, `PostToolUse`) to
    /// be transmitted without blocking the hot path of the shell or model output.
    pub async fn capture_event<P: Send + 'static>(&self, _event_name: &str, _payload: P) {
        // TODO: Transmit the event to agentmemory's ingestion endpoint.
        // The payload will typically be a hook request (e.g. `PostToolUseRequest`).
        // This is a stub for future PRs.
    }

    /// Asynchronously triggers a memory refresh/update operation in `agentmemory`.
    pub async fn update_memories(&self) -> Result<(), String> {
        // TODO: Call agentmemory's sync/refresh endpoint.
        Ok(())
    }

    /// Asynchronously drops/clears the memory store in `agentmemory`.
    pub async fn drop_memories(&self) -> Result<(), String> {
        // TODO: Call agentmemory's clear/drop endpoint.
        Ok(())
    }
}