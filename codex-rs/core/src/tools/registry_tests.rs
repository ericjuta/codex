use super::*;
use crate::agentmemory::context_planner::AgentmemoryToolCapability;
use crate::session::tests::make_session_and_context;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolCallSource;
use crate::tools::hook_names::HookToolName;
use crate::turn_diff_tracker::TurnDiffTracker;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Default)]
struct TestHandler;

impl ToolHandler for TestHandler {
    type Output = crate::tools::context::FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn pre_tool_use_payload(&self, invocation: &ToolInvocation) -> Option<PreToolUsePayload> {
        let ToolPayload::Function { arguments } = &invocation.payload else {
            return None;
        };
        Some(PreToolUsePayload {
            tool_name: HookToolName::new(invocation.tool_name.display()),
            tool_input: serde_json::from_str(arguments)
                .unwrap_or_else(|_| serde_json::json!({ "arguments": arguments })),
        })
    }

    fn post_tool_use_payload(
        &self,
        invocation: &ToolInvocation,
        result: &Self::Output,
    ) -> Option<PostToolUsePayload> {
        let ToolPayload::Function { arguments } = &invocation.payload else {
            return None;
        };
        Some(PostToolUsePayload {
            tool_name: HookToolName::new(invocation.tool_name.display()),
            tool_use_id: invocation.call_id.clone(),
            tool_input: serde_json::from_str(arguments)
                .unwrap_or_else(|_| serde_json::json!({ "arguments": arguments })),
            tool_response: result
                .post_tool_use_response(&invocation.call_id, &invocation.payload)?,
        })
    }

    async fn handle(&self, _invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        Ok(crate::tools::context::FunctionToolOutput::from_text(
            "ok".to_string(),
            Some(true),
        ))
    }
}

#[test]
fn handler_looks_up_namespaced_aliases_explicitly() {
    let plain_handler = Arc::new(TestHandler) as Arc<dyn AnyToolHandler>;
    let namespaced_handler = Arc::new(TestHandler) as Arc<dyn AnyToolHandler>;
    let namespace = "mcp__codex_apps__gmail";
    let tool_name = "gmail_get_recent_emails";
    let plain_name = codex_tools::ToolName::plain(tool_name);
    let namespaced_name = codex_tools::ToolName::namespaced(namespace, tool_name);
    let registry = ToolRegistry::new(HashMap::from([
        (plain_name.clone(), Arc::clone(&plain_handler)),
        (namespaced_name.clone(), Arc::clone(&namespaced_handler)),
    ]));

    let plain = registry.handler(&plain_name);
    let namespaced = registry.handler(&namespaced_name);
    let missing_namespaced = registry.handler(&codex_tools::ToolName::namespaced(
        "mcp__codex_apps__calendar",
        tool_name,
    ));

    assert_eq!(plain.is_some(), true);
    assert_eq!(namespaced.is_some(), true);
    assert_eq!(missing_namespaced.is_none(), true);
    assert!(
        plain
            .as_ref()
            .is_some_and(|handler| Arc::ptr_eq(handler, &plain_handler))
    );
    assert!(
        namespaced
            .as_ref()
            .is_some_and(|handler| Arc::ptr_eq(handler, &namespaced_handler))
    );
}

#[tokio::test]
async fn default_tool_handler_hook_payloads_are_retained() {
    let (session, turn) = make_session_and_context().await;
    let invocation = ToolInvocation {
        session: session.into(),
        turn: turn.into(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
        call_id: "call-1".to_string(),
        tool_name: codex_tools::ToolName::plain("example_tool"),
        source: ToolCallSource::Direct,
        payload: ToolPayload::Function {
            arguments: r#"{"alpha":1}"#.to_string(),
        },
    };

    assert_eq!(
        ToolHandler::pre_tool_use_payload(&TestHandler, &invocation),
        Some(PreToolUsePayload {
            tool_name: HookToolName::new("example_tool"),
            tool_input: serde_json::json!({"alpha": 1}),
        })
    );

    let post_payload = ToolHandler::post_tool_use_payload(
        &TestHandler,
        &invocation,
        &FunctionToolOutput::from_text("ok".to_string(), None),
    )
    .expect("default post-tool payload");

    assert_eq!(post_payload.tool_name, HookToolName::new("example_tool"));
    assert_eq!(post_payload.tool_input, serde_json::json!({"alpha": 1}));
}

#[tokio::test]
async fn default_tool_handler_populates_agentmemory_input_for_native_file_tools() {
    let (session, turn) = make_session_and_context().await;
    let invocation = ToolInvocation {
        session: session.into(),
        turn: turn.into(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
        call_id: "call-2".to_string(),
        tool_name: codex_tools::ToolName::plain("Read"),
        source: ToolCallSource::Direct,
        payload: ToolPayload::Function {
            arguments: r#"{"path":"src/main.rs"}"#.to_string(),
        },
    };

    assert_eq!(
        ToolHandler::pre_tool_use_payload(&TestHandler, &invocation),
        Some(PreToolUsePayload {
            tool_name: HookToolName::new("Read"),
            tool_input: serde_json::json!({
                "path": "src/main.rs",
            }),
        })
    );
    assert_eq!(
        AgentmemoryToolCapability::from_tool_name("Read"),
        Some(AgentmemoryToolCapability::FileRead),
    );
}
