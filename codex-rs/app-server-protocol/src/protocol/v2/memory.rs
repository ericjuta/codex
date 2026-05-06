use codex_protocol::protocol::Op as CoreOp;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMemorySubmitParams {
    pub thread_id: String,
    pub operation: ThreadMemoryOperationParams,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ThreadMemorySubmitResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type")]
#[ts(export_to = "v2/")]
pub enum ThreadMemoryOperationParams {
    Drop,
    Update,
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Recall {
        #[ts(optional = nullable)]
        query: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Remember {
        content: String,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Lessons {
        #[ts(optional = nullable)]
        query: Option<String>,
    },
    Crystals,
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    CreateCrystals {
        action_ids: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    AutoCrystallize {
        #[ts(optional = nullable)]
        older_than_days: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Reflect {
        #[ts(optional = nullable)]
        max_clusters: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Insights {
        #[ts(optional = nullable)]
        query: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    ListActions {
        #[ts(optional = nullable)]
        status: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    CreateAction {
        title: String,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    UpdateAction {
        action_id: String,
        status: String,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Missions {
        #[ts(optional = nullable)]
        mission_id: Option<String>,
        #[ts(optional = nullable)]
        status: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    BranchOverlays {
        #[ts(optional = nullable)]
        branch: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Guardrails {
        #[ts(optional = nullable)]
        query: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Decisions {
        #[ts(optional = nullable)]
        query: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Dossiers {
        #[ts(optional = nullable)]
        file_path: Option<String>,
    },
    RoutineCandidates,
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Handoffs {
        #[ts(optional = nullable)]
        handoff_packet_id: Option<String>,
        #[ts(optional = nullable)]
        scope_type: Option<String>,
        #[ts(optional = nullable)]
        scope_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    GenerateHandoff {
        #[ts(optional = nullable)]
        scope_type: Option<String>,
        #[ts(optional = nullable)]
        scope_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Frontier {
        #[ts(optional = nullable)]
        limit: Option<u32>,
    },
    Next,
}

impl ThreadMemoryOperationParams {
    pub fn from_core_op(op: &CoreOp) -> Option<Self> {
        match op {
            CoreOp::DropMemories => Some(Self::Drop),
            CoreOp::UpdateMemories => Some(Self::Update),
            CoreOp::RecallMemories { query } => Some(Self::Recall {
                query: query.clone(),
            }),
            CoreOp::RememberMemories { content } => Some(Self::Remember {
                content: content.clone(),
            }),
            CoreOp::ReviewLessons { query } => Some(Self::Lessons {
                query: query.clone(),
            }),
            CoreOp::ReviewCrystals => Some(Self::Crystals),
            CoreOp::CreateCrystals { action_ids } => Some(Self::CreateCrystals {
                action_ids: action_ids.clone(),
            }),
            CoreOp::AutoCrystallize { older_than_days } => Some(Self::AutoCrystallize {
                older_than_days: older_than_days.to_owned(),
            }),
            CoreOp::ReflectMemories { max_clusters } => Some(Self::Reflect {
                max_clusters: *max_clusters,
            }),
            CoreOp::ReviewInsights { query } => Some(Self::Insights {
                query: query.clone(),
            }),
            CoreOp::ListActions { status } => Some(Self::ListActions {
                status: status.clone(),
            }),
            CoreOp::CreateAction { title } => Some(Self::CreateAction {
                title: title.clone(),
            }),
            CoreOp::UpdateAction { action_id, status } => Some(Self::UpdateAction {
                action_id: action_id.clone(),
                status: status.clone(),
            }),
            CoreOp::ReviewMissions { mission_id, status } => Some(Self::Missions {
                mission_id: mission_id.clone(),
                status: status.clone(),
            }),
            CoreOp::ReviewBranchOverlays { branch } => Some(Self::BranchOverlays {
                branch: branch.clone(),
            }),
            CoreOp::ReviewGuardrails { query } => Some(Self::Guardrails {
                query: query.clone(),
            }),
            CoreOp::ReviewDecisions { query } => Some(Self::Decisions {
                query: query.clone(),
            }),
            CoreOp::ReviewDossiers { file_path } => Some(Self::Dossiers {
                file_path: file_path.clone(),
            }),
            CoreOp::ReviewRoutineCandidates => Some(Self::RoutineCandidates),
            CoreOp::ReviewHandoffs {
                handoff_packet_id,
                scope_type,
                scope_id,
            } => Some(Self::Handoffs {
                handoff_packet_id: handoff_packet_id.clone(),
                scope_type: scope_type.clone(),
                scope_id: scope_id.clone(),
            }),
            CoreOp::GenerateHandoff {
                scope_type,
                scope_id,
            } => Some(Self::GenerateHandoff {
                scope_type: scope_type.clone(),
                scope_id: scope_id.clone(),
            }),
            CoreOp::ReviewFrontier { limit } => Some(Self::Frontier { limit: *limit }),
            CoreOp::ReviewNext => Some(Self::Next),
            _ => None,
        }
    }

    pub fn to_core(self) -> Option<CoreOp> {
        match self {
            Self::Drop => Some(CoreOp::DropMemories),
            Self::Update => Some(CoreOp::UpdateMemories),
            Self::Recall { query } => Some(CoreOp::RecallMemories { query }),
            Self::Remember { content } => Some(CoreOp::RememberMemories { content }),
            Self::Lessons { query } => Some(CoreOp::ReviewLessons { query }),
            Self::Crystals => Some(CoreOp::ReviewCrystals),
            Self::CreateCrystals { action_ids } => Some(CoreOp::CreateCrystals { action_ids }),
            Self::AutoCrystallize { older_than_days } => {
                Some(CoreOp::AutoCrystallize { older_than_days })
            }
            Self::Reflect { max_clusters } => Some(CoreOp::ReflectMemories { max_clusters }),
            Self::Insights { query } => Some(CoreOp::ReviewInsights { query }),
            Self::ListActions { status } => Some(CoreOp::ListActions { status }),
            Self::CreateAction { title } => Some(CoreOp::CreateAction { title }),
            Self::UpdateAction { action_id, status } => {
                Some(CoreOp::UpdateAction { action_id, status })
            }
            Self::Missions { mission_id, status } => {
                Some(CoreOp::ReviewMissions { mission_id, status })
            }
            Self::Guardrails { query } => Some(CoreOp::ReviewGuardrails { query }),
            Self::Decisions { query } => Some(CoreOp::ReviewDecisions { query }),
            Self::Dossiers { file_path } => Some(CoreOp::ReviewDossiers { file_path }),
            Self::BranchOverlays { branch } => Some(CoreOp::ReviewBranchOverlays { branch }),
            Self::RoutineCandidates => Some(CoreOp::ReviewRoutineCandidates),
            Self::Handoffs {
                handoff_packet_id,
                scope_type,
                scope_id,
            } => Some(CoreOp::ReviewHandoffs {
                handoff_packet_id,
                scope_type,
                scope_id,
            }),
            Self::GenerateHandoff {
                scope_type,
                scope_id,
            } => Some(CoreOp::GenerateHandoff {
                scope_type,
                scope_id,
            }),
            Self::Frontier { limit } => Some(CoreOp::ReviewFrontier { limit }),
            Self::Next => Some(CoreOp::ReviewNext),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case", export_to = "v2/")]
pub enum MemoryOperationKind {
    Recall,
    Remember,
    Update,
    Drop,
    Lessons,
    Crystals,
    Crystallize,
    AutoCrystallize,
    Insights,
    Reflect,
    Actions,
    ActionCreate,
    ActionUpdate,
    Missions,
    Handoffs,
    HandoffGenerate,
    BranchOverlays,
    Guardrails,
    Decisions,
    Dossiers,
    RoutineCandidates,
    Frontier,
    Next,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case", export_to = "v2/")]
pub enum MemoryOperationStatus {
    Pending,
    Ready,
    Empty,
    Skipped,
    Error,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case", export_to = "v2/")]
pub enum MemoryOperationSource {
    Human,
    Assistant,
    Automatic,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case", export_to = "v2/")]
pub enum MemoryOperationScope {
    #[default]
    None,
    Turn,
    Thread,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MemoryOperationNotification {
    pub thread_id: String,
    pub source: MemoryOperationSource,
    pub operation: MemoryOperationKind,
    pub status: MemoryOperationStatus,
    #[serde(default)]
    pub scope: MemoryOperationScope,
    pub query: Option<String>,
    pub summary: String,
    pub detail: Option<String>,
    pub context_injected: bool,
}
