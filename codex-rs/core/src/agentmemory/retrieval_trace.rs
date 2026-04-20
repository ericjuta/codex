use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

const TRACE_CANDIDATE_PREVIEW_LIMIT: usize = 3;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentmemoryRetrievalTrace {
    #[serde(default)]
    pub(crate) query_terms: Vec<String>,
    #[serde(default)]
    pub(crate) lane_budgets: BTreeMap<String, usize>,
    #[serde(default)]
    pub(crate) lane_usage: BTreeMap<String, usize>,
    #[serde(default)]
    pub(crate) selected: Vec<AgentmemoryRetrievalTraceCandidate>,
    #[serde(default)]
    pub(crate) skipped: Vec<AgentmemoryRetrievalTraceCandidate>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentmemoryRetrievalTraceCandidate {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) lane: String,
    #[serde(default)]
    pub(crate) decision: String,
    #[serde(default)]
    pub(crate) preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct AgentmemoryRetrievalTraceSummary {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) query_terms: Vec<String>,
    pub(crate) selected_count: usize,
    pub(crate) skipped_count: usize,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) lane_budgets: BTreeMap<String, usize>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) lane_usage: BTreeMap<String, usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) selected: Vec<AgentmemoryRetrievalTraceCandidateSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) skipped: Vec<AgentmemoryRetrievalTraceCandidateSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct AgentmemoryRetrievalTraceCandidateSummary {
    pub(crate) id: String,
    pub(crate) lane: String,
    pub(crate) decision: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) preview: String,
}

impl From<&AgentmemoryRetrievalTrace> for AgentmemoryRetrievalTraceSummary {
    fn from(value: &AgentmemoryRetrievalTrace) -> Self {
        Self {
            query_terms: value.query_terms.clone(),
            selected_count: value.selected.len(),
            skipped_count: value.skipped.len(),
            lane_budgets: value.lane_budgets.clone(),
            lane_usage: value.lane_usage.clone(),
            selected: value
                .selected
                .iter()
                .take(TRACE_CANDIDATE_PREVIEW_LIMIT)
                .map(AgentmemoryRetrievalTraceCandidateSummary::from)
                .collect(),
            skipped: value
                .skipped
                .iter()
                .take(TRACE_CANDIDATE_PREVIEW_LIMIT)
                .map(AgentmemoryRetrievalTraceCandidateSummary::from)
                .collect(),
        }
    }
}

impl From<&AgentmemoryRetrievalTraceCandidate> for AgentmemoryRetrievalTraceCandidateSummary {
    fn from(value: &AgentmemoryRetrievalTraceCandidate) -> Self {
        Self {
            id: value.id.clone(),
            lane: value.lane.clone(),
            decision: value.decision.clone(),
            preview: value.preview.clone(),
        }
    }
}
