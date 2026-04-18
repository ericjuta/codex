use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;

use codex_protocol::items::MemoryOperationScope;
use serde::Serialize;

pub(crate) const DEFAULT_CONTEXT_BUDGET_TOKENS: usize = 2_000;
pub(crate) const QUERY_CONTEXT_BUDGET_TOKENS: usize = 2_000;
pub(crate) const PRETOOL_CONTEXT_BUDGET_TOKENS: usize = 1_200;
const MAX_AUTO_INJECTIONS_PER_TURN: usize = 4;
const REINJECT_AFTER_TURNS: u64 = 2;
const RECENT_INJECTION_HISTORY_LIMIT: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentmemoryContextReason {
    SessionStart,
    UserTurn,
    PreTool,
}

impl AgentmemoryContextReason {
    pub(crate) fn lane_key(self, detail: Option<&str>) -> String {
        match (self, detail) {
            (Self::SessionStart, _) => "session_start".to_string(),
            (Self::UserTurn, _) => "user_turn".to_string(),
            (Self::PreTool, Some(detail)) => format!("pre_tool:{detail}"),
            (Self::PreTool, None) => "pre_tool".to_string(),
        }
    }

    pub(crate) fn summary_label(self) -> &'static str {
        match self {
            Self::SessionStart => "session start",
            Self::UserTurn => "user turn",
            Self::PreTool => "pre-tool",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentmemoryContextEndpoint {
    SessionStart,
    ContextRefresh,
    Context,
    Enrich,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentmemoryContextSkipReason {
    TrivialUserTurn,
    MissingStructuredInput,
    DuplicateSuppressed,
    MaxAutoInjectionsPerTurn,
    EmptyResult,
    BackendError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentmemoryToolCapability {
    FileRead,
    FileSearch,
    FileWrite,
    Patch,
}

impl AgentmemoryToolCapability {
    pub(crate) fn from_tool_name(tool_name: &str) -> Option<Self> {
        match tool_name {
            "Read" => Some(Self::FileRead),
            "Glob" | "Grep" => Some(Self::FileSearch),
            "Edit" | "Write" => Some(Self::FileWrite),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutoInjectionRegistration {
    Allowed,
    DuplicateSuppressed,
    MaxAutoInjectionsPerTurn,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct AgentmemoryContextEventDetail {
    pub(crate) reason: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_capability: Option<AgentmemoryToolCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) endpoint: Option<AgentmemoryContextEndpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fallback_endpoint: Option<AgentmemoryContextEndpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) request_budget_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) backend_error: Option<String>,
    pub(crate) scope: MemoryOperationScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) skip_reason: Option<AgentmemoryContextSkipReason>,
    pub(crate) duplicate_suppressed: bool,
    pub(crate) fallback_used: bool,
    pub(crate) retrieval_attempted: bool,
    pub(crate) context_injected: bool,
}

impl AgentmemoryContextEventDetail {
    pub(crate) fn to_pretty_json(&self) -> Option<String> {
        serde_json::to_string_pretty(self).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecentInjectedContext {
    lane_key: String,
    context_hash: u64,
    turn_ordinal: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AgentmemoryPlannerState {
    current_turn_ordinal: u64,
    auto_injections_this_turn: usize,
    recent_injections: VecDeque<RecentInjectedContext>,
}

impl AgentmemoryPlannerState {
    pub(crate) fn begin_user_turn(&mut self) -> u64 {
        self.current_turn_ordinal += 1;
        self.auto_injections_this_turn = 0;
        self.evict_stale_injections();
        self.current_turn_ordinal
    }

    pub(crate) fn register_auto_injection(
        &mut self,
        lane_key: &str,
        context: &str,
    ) -> AutoInjectionRegistration {
        self.evict_stale_injections();
        if self.auto_injections_this_turn >= MAX_AUTO_INJECTIONS_PER_TURN {
            return AutoInjectionRegistration::MaxAutoInjectionsPerTurn;
        }

        let context_hash = hash_context(context);
        if self.recent_injections.iter().any(|recent| {
            recent.lane_key == lane_key
                && recent.context_hash == context_hash
                && self
                    .current_turn_ordinal
                    .saturating_sub(recent.turn_ordinal)
                    <= REINJECT_AFTER_TURNS
        }) {
            return AutoInjectionRegistration::DuplicateSuppressed;
        }

        self.auto_injections_this_turn += 1;
        self.recent_injections.push_back(RecentInjectedContext {
            lane_key: lane_key.to_string(),
            context_hash,
            turn_ordinal: self.current_turn_ordinal,
        });
        while self.recent_injections.len() > RECENT_INJECTION_HISTORY_LIMIT {
            self.recent_injections.pop_front();
        }
        AutoInjectionRegistration::Allowed
    }

    fn evict_stale_injections(&mut self) {
        while let Some(front) = self.recent_injections.front() {
            if self.current_turn_ordinal.saturating_sub(front.turn_ordinal) > REINJECT_AFTER_TURNS {
                self.recent_injections.pop_front();
            } else {
                break;
            }
        }
    }
}

pub(crate) fn is_trivial_user_turn(prompt: &str) -> bool {
    let normalized = normalize_prompt_for_triviality(prompt);
    if normalized.is_empty() {
        return true;
    }

    matches!(
        normalized.as_str(),
        "ok" | "okay"
            | "thanks"
            | "thank you"
            | "thx"
            | "continue"
            | "proceed"
            | "go on"
            | "yep"
            | "yes"
            | "sure"
    )
}

fn normalize_prompt_for_triviality(prompt: &str) -> String {
    prompt
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch.is_ascii_whitespace() {
                ch
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn hash_context(context: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    context.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn trivial_user_turn_detection_matches_expected_acks() {
        assert_eq!(is_trivial_user_turn("ok"), true);
        assert_eq!(is_trivial_user_turn("Thanks!"), true);
        assert_eq!(is_trivial_user_turn("continue"), true);
        assert_eq!(is_trivial_user_turn("fix the failing test"), false);
        assert_eq!(is_trivial_user_turn("read src/main.rs"), false);
    }

    #[test]
    fn planner_state_suppresses_exact_duplicates_within_window() {
        let mut state = AgentmemoryPlannerState::default();
        state.begin_user_turn();
        assert_eq!(
            state.register_auto_injection(
                "user_turn",
                "<agentmemory-context>a</agentmemory-context>"
            ),
            AutoInjectionRegistration::Allowed,
        );
        state.begin_user_turn();
        assert_eq!(
            state.register_auto_injection(
                "user_turn",
                "<agentmemory-context>a</agentmemory-context>"
            ),
            AutoInjectionRegistration::DuplicateSuppressed,
        );
        state.begin_user_turn();
        assert_eq!(
            state.register_auto_injection(
                "user_turn",
                "<agentmemory-context>b</agentmemory-context>"
            ),
            AutoInjectionRegistration::Allowed,
        );
        state.begin_user_turn();
        assert_eq!(
            state.register_auto_injection(
                "user_turn",
                "<agentmemory-context>a</agentmemory-context>"
            ),
            AutoInjectionRegistration::Allowed,
        );
    }
}
