use super::*;
use crate::codex::make_session_and_context;
use crate::tools::context::FunctionToolOutput;
use crate::turn_diff_tracker::TurnDiffTracker;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tokio::sync::Mutex;

struct TestHandler;

impl ToolHandler for TestHandler {
    type Output = crate::tools::context::FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, _invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        unreachable!("test handler should not be invoked")
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
        tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
        call_id: "call-1".to_string(),
        tool_name: codex_tools::ToolName::plain("example_tool"),
        payload: ToolPayload::Function {
            arguments: r#"{"alpha":1}"#.to_string(),
        },
    };

    assert_eq!(
        ToolHandler::pre_tool_use_payload(&TestHandler, &invocation),
        Some(PreToolUsePayload {
            tool_name: "example_tool".to_string(),
            command: r#"{"alpha":1}"#.to_string(),
            agentmemory_input: None,
        })
    );

    let post_payload = ToolHandler::post_tool_use_payload(
        &TestHandler,
        &invocation,
        &FunctionToolOutput::from_text("ok".to_string(), None),
    )
    .expect("default post-tool payload");

    assert_eq!(post_payload.tool_name, "example_tool");
    assert_eq!(post_payload.command, r#"{"alpha":1}"#);
}
