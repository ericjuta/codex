use crate::shell::Shell;
use crate::shell::ShellType;
use crate::tools::handlers::agent_jobs::BatchJobHandler;
use crate::tools::handlers::multi_agents_common::DEFAULT_WAIT_TIMEOUT_MS;
use crate::tools::handlers::multi_agents_common::MAX_WAIT_TIMEOUT_MS;
use crate::tools::handlers::multi_agents_common::MIN_WAIT_TIMEOUT_MS;
use crate::tools::registry::ToolRegistryBuilder;
use codex_config::types::MemoryBackend;
use codex_mcp::ToolInfo;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_tools::AdditionalProperties;
use codex_tools::DiscoverableTool;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolHandlerKind;
use codex_tools::ToolName;
use codex_tools::ToolNamespace;
use codex_tools::ToolRegistryPlanDeferredTool;
use codex_tools::ToolRegistryPlanMcpTool;
use codex_tools::ToolRegistryPlanParams;
use codex_tools::ToolSpec;
use codex_tools::ToolUserShellType;
use codex_tools::ToolsConfig;
use codex_tools::WaitAgentTimeoutOptions;
use codex_tools::augment_tool_spec_for_code_mode;
use codex_tools::build_tool_registry_plan;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

pub(crate) fn tool_user_shell_type(user_shell: &Shell) -> ToolUserShellType {
    match user_shell.shell_type {
        ShellType::Zsh => ToolUserShellType::Zsh,
        ShellType::Bash => ToolUserShellType::Bash,
        ShellType::PowerShell => ToolUserShellType::PowerShell,
        ShellType::Sh => ToolUserShellType::Sh,
        ShellType::Cmd => ToolUserShellType::Cmd,
    }
}

fn memory_recall_output_schema() -> JsonValue {
    json!({
        "type": "object",
        "properties": {
            "recalled": {
                "type": "boolean",
                "description": "Whether agentmemory returned any context for this request."
            },
            "context": {
                "type": "string",
                "description": "Recalled memory context. Empty when nothing relevant was found."
            },
            "scope": {
                "type": "string",
                "enum": ["turn", "thread"],
                "description": "The scope applied to the recalled context."
            }
        },
        "required": ["recalled", "context", "scope"],
        "additionalProperties": false
    })
}

fn memory_success_output_schema(primary_key: &str) -> JsonValue {
    json!({
        "type": "object",
        "properties": {
            "success": {
                "type": "boolean"
            },
            primary_key: {
                "type": ["array", "object", "null"]
            },
            "error": {
                "type": ["string", "null"]
            }
        },
        "required": ["success"],
        "additionalProperties": true
    })
}

fn create_memory_remember_tool() -> ToolSpec {
    let properties = std::collections::BTreeMap::from([(
        "content".to_string(),
        JsonSchema::string(Some(
            "Durable memory content to save explicitly for later recall.".to_string(),
        )),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_remember".to_string(),
        description: "Persist durable, high-value knowledge into agentmemory. Use this sparingly for facts, patterns, decisions, or lessons that should survive beyond the current turn."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["content".to_string()]),
            Some(false.into()),
        ),
        output_schema: Some(memory_success_output_schema("memory")),
    })
}

fn create_memory_lessons_tool() -> ToolSpec {
    let properties = std::collections::BTreeMap::from([(
        "query".to_string(),
        JsonSchema::string(Some(
            "Optional targeted lesson search query. When omitted, returns the current lesson list for this project."
                .to_string(),
        )),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_lessons".to_string(),
        description: "Review lessons derived by agentmemory for the current project. Use a query to search, or omit it to list current lessons."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: Some(memory_success_output_schema("lessons")),
    })
}

fn create_memory_crystals_tool() -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: "memory_crystals".to_string(),
        description: "Review crystallized action-chain digests for the current project."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(std::collections::BTreeMap::new(), None, Some(false.into())),
        output_schema: Some(memory_success_output_schema("crystals")),
    })
}

fn create_memory_insights_tool() -> ToolSpec {
    let properties = std::collections::BTreeMap::from([(
        "query".to_string(),
        JsonSchema::string(Some(
            "Optional targeted insight search query. When omitted, returns the current insight list for this project."
                .to_string(),
        )),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_insights".to_string(),
        description: "Review higher-level insights derived by agentmemory for the current project. Use a query to search, or omit it to list current insights."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: Some(memory_success_output_schema("insights")),
    })
}

fn create_memory_actions_tool() -> ToolSpec {
    let properties = std::collections::BTreeMap::from([(
        "status".to_string(),
        JsonSchema::string(Some(
            "Optional action status filter such as pending, active, blocked, done, or cancelled."
                .to_string(),
        )),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_actions".to_string(),
        description:
            "Review explicit action work items tracked by agentmemory for the current project."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: Some(memory_success_output_schema("actions")),
    })
}

fn create_memory_missions_tool() -> ToolSpec {
    let properties = std::collections::BTreeMap::from([
        (
            "mission_id".to_string(),
            JsonSchema::string(Some(
                "Optional mission id to fetch directly. When omitted, returns missions for the current project."
                    .to_string(),
            )),
        ),
        (
            "status".to_string(),
            JsonSchema::string(Some(
                "Optional mission status filter such as draft, active, blocked, completed, or cancelled."
                    .to_string(),
            )),
        ),
        (
            "owner".to_string(),
            JsonSchema::string(Some(
                "Optional mission owner filter.".to_string(),
            )),
        ),
        (
            "limit".to_string(),
            JsonSchema::number(Some(
                "Optional maximum number of missions to return.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_missions".to_string(),
        description: "Review mission containers tracked by agentmemory for the current project."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: Some(json!({
            "type": "object",
            "properties": {
                "success": { "type": "boolean" },
                "mission": { "type": ["object", "null"] },
                "missions": { "type": ["array", "null"] },
                "statusSummary": { "type": ["object", "null"] },
                "error": { "type": ["string", "null"] }
            },
            "required": ["success"],
            "additionalProperties": true
        })),
    })
}

fn create_memory_handoffs_tool() -> ToolSpec {
    let properties = std::collections::BTreeMap::from([
        (
            "handoff_packet_id".to_string(),
            JsonSchema::string(Some(
                "Optional handoff packet id to fetch directly. When omitted, returns handoff packets for the current project."
                    .to_string(),
            )),
        ),
        (
            "scope_type".to_string(),
            JsonSchema::string_enum(
                vec![json!("action"), json!("mission"), json!("session")],
                Some("Optional handoff scope type filter.".to_string()),
            ),
        ),
        (
            "scope_id".to_string(),
            JsonSchema::string(Some(
                "Optional handoff scope id filter.".to_string(),
            )),
        ),
        (
            "limit".to_string(),
            JsonSchema::number(Some(
                "Optional maximum number of handoff packets to return.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_handoffs".to_string(),
        description:
            "Review durable handoff packets tracked by agentmemory for the current project."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: Some(json!({
            "type": "object",
            "properties": {
                "success": { "type": "boolean" },
                "handoffPacket": { "type": ["object", "null"] },
                "handoffPackets": { "type": ["array", "null"] },
                "error": { "type": ["string", "null"] }
            },
            "required": ["success"],
            "additionalProperties": true
        })),
    })
}

fn create_memory_branch_overlays_tool() -> ToolSpec {
    let properties = std::collections::BTreeMap::from([
        (
            "branch".to_string(),
            JsonSchema::string(Some(
                "Optional branch filter. When omitted, returns branch overlays for the current project."
                    .to_string(),
            )),
        ),
        (
            "limit".to_string(),
            JsonSchema::number(Some(
                "Optional maximum number of overlays to return.".to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_branch_overlays".to_string(),
        description:
            "Review branch-scoped overlay notes tracked by agentmemory for the current project."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: Some(memory_success_output_schema("overlays")),
    })
}

fn create_memory_guardrails_tool() -> ToolSpec {
    let properties = std::collections::BTreeMap::from([(
        "query".to_string(),
        JsonSchema::string(Some(
            "Optional targeted guardrail search query. When omitted, returns the current guardrail list for this project."
                .to_string(),
        )),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_guardrails".to_string(),
        description: "Review guardrails derived by agentmemory for the current project. Use a query to search, or omit it to list current guardrails."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: Some(memory_success_output_schema("guardrails")),
    })
}

fn create_memory_decisions_tool() -> ToolSpec {
    let properties = std::collections::BTreeMap::from([(
        "query".to_string(),
        JsonSchema::string(Some(
            "Optional targeted decision search query. When omitted, returns the current decision list for this project."
                .to_string(),
        )),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_decisions".to_string(),
        description: "Review decision memory derived by agentmemory for the current project. Use a query to search, or omit it to list current decisions."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: Some(memory_success_output_schema("decisions")),
    })
}

fn create_memory_dossiers_tool() -> ToolSpec {
    let properties = std::collections::BTreeMap::from([
        (
            "file_path".to_string(),
            JsonSchema::string(Some(
                "Optional file path to fetch a specific dossier. When omitted, returns the dossier list for the current project."
                    .to_string(),
            )),
        ),
        (
            "refresh".to_string(),
            JsonSchema::boolean(Some(
                "When true and file_path is provided, refresh the dossier before returning it."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_dossiers".to_string(),
        description:
            "Review file-level component dossiers tracked by agentmemory for the current project."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: Some(json!({
            "type": "object",
            "properties": {
                "success": { "type": "boolean" },
                "dossier": { "type": ["object", "null"] },
                "dossiers": { "type": ["array", "null"] },
                "error": { "type": ["string", "null"] }
            },
            "required": ["success"],
            "additionalProperties": true
        })),
    })
}

fn create_memory_routine_candidates_tool() -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: "memory_routine_candidates".to_string(),
        description:
            "Review proposal-only routine candidates mined from repeated successful action chains."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(std::collections::BTreeMap::new(), None, Some(false.into())),
        output_schema: Some(memory_success_output_schema("routineCandidates")),
    })
}

fn create_memory_handoff_generate_tool() -> ToolSpec {
    let properties = std::collections::BTreeMap::from([
        (
            "scope_type".to_string(),
            JsonSchema::string_enum(
                vec![json!("action"), json!("mission"), json!("session")],
                Some(
                    "Optional handoff scope type. Defaults to `session`.".to_string(),
                ),
            ),
        ),
        (
            "scope_id".to_string(),
            JsonSchema::string(Some(
                "Optional scope id. Defaults to the current session id when scope_type is `session`."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_handoff_generate".to_string(),
        description:
            "Generate a fresh durable handoff packet from agentmemory for the current project."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: Some(json!({
            "type": "object",
            "properties": {
                "success": { "type": "boolean" },
                "handoffPacket": { "type": ["object", "null"] },
                "signal": { "type": ["object", "null"] },
                "error": { "type": ["string", "null"] }
            },
            "required": ["success"],
            "additionalProperties": true
        })),
    })
}

fn create_memory_frontier_tool() -> ToolSpec {
    let properties = std::collections::BTreeMap::from([(
        "limit".to_string(),
        JsonSchema::number(Some(
            "Optional maximum number of frontier suggestions to return.".to_string(),
        )),
    )]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_frontier".to_string(),
        description:
            "Review the current frontier of unblocked action suggestions from agentmemory."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, None, Some(false.into())),
        output_schema: Some(memory_success_output_schema("frontier")),
    })
}

fn create_memory_next_tool() -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: "memory_next".to_string(),
        description: "Review the single best next action suggestion from agentmemory for the current project."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(std::collections::BTreeMap::new(), None, Some(false.into())),
        output_schema: Some(memory_success_output_schema("suggestion")),
    })
}

fn create_memory_recall_tool() -> ToolSpec {
    let properties = std::collections::BTreeMap::from([
        (
            "query".to_string(),
            JsonSchema::string(Some(
                "Optional targeted memory recall query. When omitted, recall uses the current thread and project context only."
                    .to_string(),
            )),
        ),
        (
            "scope".to_string(),
            JsonSchema::string_enum(
                vec![json!("turn"), json!("thread")],
                Some(
                    "Optional recall scope. Defaults to `turn`; use `thread` to persist the recalled context into conversation history."
                        .to_string(),
                ),
            ),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: "memory_recall".to_string(),
        description: "Recall relevant agentmemory context for the current thread and project. Use this when prior work, design rationale, earlier failures, or cross-session continuity matter and the current thread does not already contain enough context. Prefer targeted queries naming the feature, file, bug, or decision you need."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, /*required*/ None, Some(false.into())),
        output_schema: Some(memory_recall_output_schema()),
    })
}

struct McpToolPlanInputs<'a> {
    mcp_tools: Vec<ToolRegistryPlanMcpTool<'a>>,
    tool_namespaces: HashMap<String, ToolNamespace>,
}

fn map_mcp_tools_for_plan(mcp_tools: &HashMap<String, ToolInfo>) -> McpToolPlanInputs<'_> {
    McpToolPlanInputs {
        mcp_tools: mcp_tools
            .values()
            .map(|tool| ToolRegistryPlanMcpTool {
                name: tool.canonical_tool_name(),
                tool: &tool.tool,
            })
            .collect(),
        tool_namespaces: mcp_tools
            .values()
            .map(|tool| {
                (
                    tool.callable_namespace.clone(),
                    ToolNamespace {
                        name: tool.callable_namespace.clone(),
                        description: tool
                            .connector_description
                            .clone()
                            .or_else(|| tool.server_instructions.clone()),
                    },
                )
            })
            .collect(),
    }
}

pub(crate) fn build_specs_with_discoverable_tools(
    config: &ToolsConfig,
    mcp_tools: Option<HashMap<String, ToolInfo>>,
    deferred_mcp_tools: Option<HashMap<String, ToolInfo>>,
    unavailable_called_tools: Vec<ToolName>,
    discoverable_tools: Option<Vec<DiscoverableTool>>,
    dynamic_tools: &[DynamicToolSpec],
) -> ToolRegistryBuilder {
    use crate::tools::handlers::ApplyPatchHandler;
    use crate::tools::handlers::CodeModeExecuteHandler;
    use crate::tools::handlers::CodeModeWaitHandler;
    use crate::tools::handlers::DynamicToolHandler;
    use crate::tools::handlers::GoalHandler;
    use crate::tools::handlers::ListDirHandler;
    use crate::tools::handlers::McpHandler;
    use crate::tools::handlers::McpResourceHandler;
    use crate::tools::handlers::MemoryActionsHandler;
    use crate::tools::handlers::MemoryBranchOverlaysHandler;
    use crate::tools::handlers::MemoryCrystalsHandler;
    use crate::tools::handlers::MemoryDecisionsHandler;
    use crate::tools::handlers::MemoryDossiersHandler;
    use crate::tools::handlers::MemoryFrontierHandler;
    use crate::tools::handlers::MemoryGuardrailsHandler;
    use crate::tools::handlers::MemoryHandoffGenerateHandler;
    use crate::tools::handlers::MemoryHandoffsHandler;
    use crate::tools::handlers::MemoryInsightsHandler;
    use crate::tools::handlers::MemoryLessonsHandler;
    use crate::tools::handlers::MemoryMissionsHandler;
    use crate::tools::handlers::MemoryNextHandler;
    use crate::tools::handlers::MemoryRecallHandler;
    use crate::tools::handlers::MemoryRememberHandler;
    use crate::tools::handlers::MemoryRoutineCandidatesHandler;
    use crate::tools::handlers::PlanHandler;
    use crate::tools::handlers::RequestPermissionsHandler;
    use crate::tools::handlers::RequestUserInputHandler;
    use crate::tools::handlers::ShellCommandHandler;
    use crate::tools::handlers::ShellHandler;
    use crate::tools::handlers::TestSyncHandler;
    use crate::tools::handlers::ToolSearchHandler;
    use crate::tools::handlers::ToolSuggestHandler;
    use crate::tools::handlers::UnavailableToolHandler;
    use crate::tools::handlers::UnifiedExecHandler;
    use crate::tools::handlers::ViewImageHandler;
    use crate::tools::handlers::multi_agents::CloseAgentHandler;
    use crate::tools::handlers::multi_agents::ResumeAgentHandler;
    use crate::tools::handlers::multi_agents::SendInputHandler;
    use crate::tools::handlers::multi_agents::SpawnAgentHandler;
    use crate::tools::handlers::multi_agents::WaitAgentHandler;
    use crate::tools::handlers::multi_agents_v2::CloseAgentHandler as CloseAgentHandlerV2;
    use crate::tools::handlers::multi_agents_v2::FollowupTaskHandler as FollowupTaskHandlerV2;
    use crate::tools::handlers::multi_agents_v2::ListAgentsHandler as ListAgentsHandlerV2;
    use crate::tools::handlers::multi_agents_v2::SendMessageHandler as SendMessageHandlerV2;
    use crate::tools::handlers::multi_agents_v2::SpawnAgentHandler as SpawnAgentHandlerV2;
    use crate::tools::handlers::multi_agents_v2::WaitAgentHandler as WaitAgentHandlerV2;
    use crate::tools::handlers::unavailable_tool_message;
    use crate::tools::tool_search_entry::build_tool_search_entries_for_config;

    let mut builder = ToolRegistryBuilder::new();
    let mcp_tool_plan_inputs = mcp_tools.as_ref().map(map_mcp_tools_for_plan);
    let deferred_mcp_tool_sources = deferred_mcp_tools.as_ref().map(|tools| {
        tools
            .values()
            .map(|tool| ToolRegistryPlanDeferredTool {
                name: tool.canonical_tool_name(),
                server_name: tool.server_name.as_str(),
                connector_name: tool.connector_name.as_deref(),
                connector_description: tool.connector_description.as_deref(),
            })
            .collect::<Vec<_>>()
    });
    let default_agent_type_description =
        crate::agent::role::spawn_tool_spec::build(&std::collections::BTreeMap::new());
    let min_wait_timeout_ms = if config.multi_agent_v2 {
        config
            .wait_agent_min_timeout_ms
            .unwrap_or(MIN_WAIT_TIMEOUT_MS)
            .clamp(1, MAX_WAIT_TIMEOUT_MS)
    } else {
        MIN_WAIT_TIMEOUT_MS
    };
    let default_wait_timeout_ms =
        DEFAULT_WAIT_TIMEOUT_MS.clamp(min_wait_timeout_ms, MAX_WAIT_TIMEOUT_MS);
    let plan = build_tool_registry_plan(
        config,
        ToolRegistryPlanParams {
            mcp_tools: mcp_tool_plan_inputs
                .as_ref()
                .map(|inputs| inputs.mcp_tools.as_slice()),
            deferred_mcp_tools: deferred_mcp_tool_sources.as_deref(),
            tool_namespaces: mcp_tool_plan_inputs
                .as_ref()
                .map(|inputs| &inputs.tool_namespaces),
            discoverable_tools: discoverable_tools.as_deref(),
            dynamic_tools,
            default_agent_type_description: &default_agent_type_description,
            wait_agent_timeouts: WaitAgentTimeoutOptions {
                default_timeout_ms: default_wait_timeout_ms,
                min_timeout_ms: min_wait_timeout_ms,
                max_timeout_ms: MAX_WAIT_TIMEOUT_MS,
            },
        },
    );
    let shell_handler = Arc::new(ShellHandler);
    let unified_exec_handler = Arc::new(UnifiedExecHandler);
    let plan_handler = Arc::new(PlanHandler);
    let apply_patch_handler = Arc::new(ApplyPatchHandler);
    let dynamic_tool_handler = Arc::new(DynamicToolHandler);
    let goal_handler = Arc::new(GoalHandler);
    let view_image_handler = Arc::new(ViewImageHandler);
    let mcp_handler = Arc::new(McpHandler);
    let mcp_resource_handler = Arc::new(McpResourceHandler);
    let shell_command_handler = Arc::new(ShellCommandHandler::from(config.shell_command_backend));
    let request_permissions_handler = Arc::new(RequestPermissionsHandler);
    let request_user_input_handler = Arc::new(RequestUserInputHandler {
        default_mode_request_user_input: config.default_mode_request_user_input,
    });
    let deferred_dynamic_tools = dynamic_tools
        .iter()
        .filter(|tool| tool.defer_loading && (config.namespace_tools || tool.namespace.is_none()))
        .cloned()
        .collect::<Vec<_>>();
    let memory_actions_handler = Arc::new(MemoryActionsHandler);
    let memory_branch_overlays_handler = Arc::new(MemoryBranchOverlaysHandler);
    let memory_crystals_handler = Arc::new(MemoryCrystalsHandler);
    let memory_decisions_handler = Arc::new(MemoryDecisionsHandler);
    let memory_dossiers_handler = Arc::new(MemoryDossiersHandler);
    let memory_frontier_handler = Arc::new(MemoryFrontierHandler);
    let memory_guardrails_handler = Arc::new(MemoryGuardrailsHandler);
    let memory_handoff_generate_handler = Arc::new(MemoryHandoffGenerateHandler);
    let memory_handoffs_handler = Arc::new(MemoryHandoffsHandler);
    let memory_insights_handler = Arc::new(MemoryInsightsHandler);
    let memory_lessons_handler = Arc::new(MemoryLessonsHandler);
    let memory_missions_handler = Arc::new(MemoryMissionsHandler);
    let memory_next_handler = Arc::new(MemoryNextHandler);
    let memory_recall_handler = Arc::new(MemoryRecallHandler);
    let memory_remember_handler = Arc::new(MemoryRememberHandler);
    let memory_routine_candidates_handler = Arc::new(MemoryRoutineCandidatesHandler);
    let mut tool_search_handler = None;
    let tool_suggest_handler = Arc::new(ToolSuggestHandler);
    let code_mode_handler = Arc::new(CodeModeExecuteHandler);
    let code_mode_wait_handler = Arc::new(CodeModeWaitHandler);
    let unavailable_tool_handler = Arc::new(UnavailableToolHandler);
    let mut existing_spec_names = plan
        .specs
        .iter()
        .map(|configured_tool| configured_tool.name().to_string())
        .collect::<HashSet<_>>();

    for spec in plan.specs {
        if spec.supports_parallel_tool_calls {
            builder.push_spec_with_parallel_support(
                spec.spec, /*supports_parallel_tool_calls*/ true,
            );
        } else {
            builder.push_spec(spec.spec);
        }
    }

    for handler in plan.handlers {
        match handler.kind {
            ToolHandlerKind::AgentJobs => {
                builder.register_handler(handler.name, Arc::new(BatchJobHandler));
            }
            ToolHandlerKind::ApplyPatch => {
                builder.register_handler(handler.name, apply_patch_handler.clone());
            }
            ToolHandlerKind::CloseAgentV1 => {
                builder.register_handler(handler.name, Arc::new(CloseAgentHandler));
            }
            ToolHandlerKind::CloseAgentV2 => {
                builder.register_handler(handler.name, Arc::new(CloseAgentHandlerV2));
            }
            ToolHandlerKind::CodeModeExecute => {
                builder.register_handler(handler.name, code_mode_handler.clone());
            }
            ToolHandlerKind::CodeModeWait => {
                builder.register_handler(handler.name, code_mode_wait_handler.clone());
            }
            ToolHandlerKind::DynamicTool => {
                builder.register_handler(handler.name, dynamic_tool_handler.clone());
            }
            ToolHandlerKind::FollowupTaskV2 => {
                builder.register_handler(handler.name, Arc::new(FollowupTaskHandlerV2));
            }
            ToolHandlerKind::Goal => {
                builder.register_handler(handler.name, goal_handler.clone());
            }
            ToolHandlerKind::ListAgentsV2 => {
                builder.register_handler(handler.name, Arc::new(ListAgentsHandlerV2));
            }
            ToolHandlerKind::ListDir => {
                builder.register_handler(handler.name, Arc::new(ListDirHandler));
            }
            ToolHandlerKind::Mcp => {
                builder.register_handler(handler.name, mcp_handler.clone());
            }
            ToolHandlerKind::McpResource => {
                builder.register_handler(handler.name, mcp_resource_handler.clone());
            }
            ToolHandlerKind::Plan => {
                builder.register_handler(handler.name, plan_handler.clone());
            }
            ToolHandlerKind::RequestPermissions => {
                builder.register_handler(handler.name, request_permissions_handler.clone());
            }
            ToolHandlerKind::RequestUserInput => {
                builder.register_handler(handler.name, request_user_input_handler.clone());
            }
            ToolHandlerKind::ResumeAgentV1 => {
                builder.register_handler(handler.name, Arc::new(ResumeAgentHandler));
            }
            ToolHandlerKind::SendInputV1 => {
                builder.register_handler(handler.name, Arc::new(SendInputHandler));
            }
            ToolHandlerKind::SendMessageV2 => {
                builder.register_handler(handler.name, Arc::new(SendMessageHandlerV2));
            }
            ToolHandlerKind::Shell => {
                builder.register_handler(handler.name, shell_handler.clone());
            }
            ToolHandlerKind::ShellCommand => {
                builder.register_handler(handler.name, shell_command_handler.clone());
            }
            ToolHandlerKind::SpawnAgentV1 => {
                builder.register_handler(handler.name, Arc::new(SpawnAgentHandler));
            }
            ToolHandlerKind::SpawnAgentV2 => {
                builder.register_handler(handler.name, Arc::new(SpawnAgentHandlerV2));
            }
            ToolHandlerKind::TestSync => {
                builder.register_handler(handler.name, Arc::new(TestSyncHandler));
            }
            ToolHandlerKind::ToolSearch => {
                if tool_search_handler.is_none() {
                    let entries = build_tool_search_entries_for_config(
                        config,
                        deferred_mcp_tools.as_ref(),
                        &deferred_dynamic_tools,
                    );
                    tool_search_handler = Some(Arc::new(ToolSearchHandler::new(entries)));
                }
                if let Some(tool_search_handler) = tool_search_handler.as_ref() {
                    builder.register_handler(handler.name, tool_search_handler.clone());
                }
            }
            ToolHandlerKind::ToolSuggest => {
                builder.register_handler(handler.name, tool_suggest_handler.clone());
            }
            ToolHandlerKind::UnifiedExec => {
                builder.register_handler(handler.name, unified_exec_handler.clone());
            }
            ToolHandlerKind::ViewImage => {
                builder.register_handler(handler.name, view_image_handler.clone());
            }
            ToolHandlerKind::WaitAgentV1 => {
                builder.register_handler(handler.name, Arc::new(WaitAgentHandler));
            }
            ToolHandlerKind::WaitAgentV2 => {
                builder.register_handler(handler.name, Arc::new(WaitAgentHandlerV2));
            }
        }
    }
    if config.memory_tool_enabled && config.memory_backend == MemoryBackend::Agentmemory {
        builder.push_spec(create_memory_recall_tool());
        builder.push_spec(create_memory_remember_tool());
        builder.push_spec(create_memory_lessons_tool());
        builder.push_spec(create_memory_crystals_tool());
        builder.push_spec(create_memory_insights_tool());
        builder.push_spec(create_memory_actions_tool());
        builder.push_spec(create_memory_missions_tool());
        builder.push_spec(create_memory_handoffs_tool());
        builder.push_spec(create_memory_branch_overlays_tool());
        builder.push_spec(create_memory_guardrails_tool());
        builder.push_spec(create_memory_decisions_tool());
        builder.push_spec(create_memory_dossiers_tool());
        builder.push_spec(create_memory_routine_candidates_tool());
        builder.push_spec(create_memory_handoff_generate_tool());
        builder.push_spec(create_memory_frontier_tool());
        builder.push_spec(create_memory_next_tool());
        builder.register_handler("memory_actions", memory_actions_handler);
        builder.register_handler("memory_branch_overlays", memory_branch_overlays_handler);
        builder.register_handler("memory_crystals", memory_crystals_handler);
        builder.register_handler("memory_decisions", memory_decisions_handler);
        builder.register_handler("memory_dossiers", memory_dossiers_handler);
        builder.register_handler("memory_frontier", memory_frontier_handler);
        builder.register_handler("memory_guardrails", memory_guardrails_handler);
        builder.register_handler("memory_handoff_generate", memory_handoff_generate_handler);
        builder.register_handler("memory_handoffs", memory_handoffs_handler);
        builder.register_handler("memory_insights", memory_insights_handler);
        builder.register_handler("memory_lessons", memory_lessons_handler);
        builder.register_handler("memory_missions", memory_missions_handler);
        builder.register_handler("memory_next", memory_next_handler);
        builder.register_handler("memory_recall", memory_recall_handler);
        builder.register_handler("memory_remember", memory_remember_handler);
        builder.register_handler(
            "memory_routine_candidates",
            memory_routine_candidates_handler,
        );
    }
    if let Some(deferred_mcp_tools) = deferred_mcp_tools.as_ref() {
        for (name, _) in deferred_mcp_tools.iter().filter(|(name, _)| {
            !mcp_tools
                .as_ref()
                .is_some_and(|tools| tools.contains_key(*name))
        }) {
            builder.register_handler(name.clone(), mcp_handler.clone());
        }
    }

    for unavailable_tool in unavailable_called_tools {
        let tool_name = unavailable_tool.display();
        if existing_spec_names.insert(tool_name.clone()) {
            let spec = codex_tools::ToolSpec::Function(ResponsesApiTool {
                name: tool_name.clone(),
                description: unavailable_tool_message(
                    &tool_name,
                    "Calling this placeholder returns an error explaining that the tool is unavailable.",
                ),
                strict: false,
                parameters: JsonSchema::object(
                    Default::default(),
                    /*required*/ None,
                    Some(AdditionalProperties::Boolean(false)),
                ),
                output_schema: None,
                defer_loading: None,
            });
            let spec = if config.code_mode_enabled {
                augment_tool_spec_for_code_mode(spec)
            } else {
                spec
            };
            builder.push_spec(spec);
        }
        builder.register_handler(unavailable_tool, unavailable_tool_handler.clone());
    }
    builder
}

#[cfg(test)]
#[path = "spec_tests.rs"]
mod tests;
