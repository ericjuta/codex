use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookOutputEntryKind;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::protocol::HookRunSummary;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::hooks::trust_discovered_hooks;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::test_codex::turn_permission_fields;
use core_test_support::wait_for_event_with_timeout;
use pretty_assertions::assert_eq;
use serde_json::Value;

const SESSION_CONTEXT_A: &str = "smoke-session-context-a";
const SESSION_CONTEXT_B: &str = "smoke-session-context-b";
const USER_PROMPT_CONTEXT: &str = "smoke-user-prompt-context";
const POST_TOOL_CONTEXT: &str = "smoke-post-tool-context";
const PRE_BLOCK_REASON: &str = "smoke pre hook blocked this command";
const PERMISSION_DENY_REASON: &str = "smoke permission hook denied this command";
const STOP_CONTINUATION_PROMPT: &str = "smoke stop hook continuation prompt";

#[tokio::test]
async fn operator_smoke_pack_covers_supported_command_hooks() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let pre_block_call_id = "smoke-pre-block";
    let permission_allow_call_id = "smoke-permission-allow";
    let permission_deny_call_id = "smoke-permission-deny";
    let pre_block_marker = std::env::temp_dir().join(format!(
        "SMOKE_PRE_BLOCK-codex-hooks-smoke-{}",
        std::process::id()
    ));
    let permission_allow_marker = std::env::temp_dir().join(format!(
        "SMOKE_PERMISSION_ALLOW-codex-hooks-smoke-{}",
        std::process::id()
    ));
    let permission_deny_marker = std::env::temp_dir().join(format!(
        "SMOKE_PERMISSION_DENY-codex-hooks-smoke-{}",
        std::process::id()
    ));
    remove_if_exists(&pre_block_marker)?;
    remove_if_exists(&permission_allow_marker)?;
    remove_if_exists(&permission_deny_marker)?;
    fs::write(&pre_block_marker, "seed").context("create pre block marker")?;
    fs::write(&permission_allow_marker, "seed").context("create permission allow marker")?;
    fs::write(&permission_deny_marker, "seed").context("create permission deny marker")?;

    let pre_block_command = format!("rm -f {}", pre_block_marker.display());
    let permission_allow_command = format!("rm -f {}", permission_allow_marker.display());
    let permission_deny_command = format!("rm -f {}", permission_deny_marker.display());

    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-smoke-pre-block"),
                ev_function_call(
                    pre_block_call_id,
                    "shell_command",
                    &serde_json::to_string(&serde_json::json!({
                        "command": pre_block_command,
                    }))?,
                ),
                ev_completed("resp-smoke-pre-block"),
            ]),
            sse(vec![
                ev_response_created("resp-smoke-permission-allow"),
                ev_function_call(
                    permission_allow_call_id,
                    "shell_command",
                    &serde_json::to_string(&serde_json::json!({
                        "command": permission_allow_command,
                    }))?,
                ),
                ev_completed("resp-smoke-permission-allow"),
            ]),
            sse(vec![
                ev_response_created("resp-smoke-permission-deny"),
                ev_function_call(
                    permission_deny_call_id,
                    "shell_command",
                    &serde_json::to_string(&serde_json::json!({
                        "command": permission_deny_command,
                    }))?,
                ),
                ev_completed("resp-smoke-permission-deny"),
            ]),
            sse(vec![
                ev_response_created("resp-smoke-draft"),
                ev_assistant_message("msg-smoke-draft", "draft before stop hook"),
                ev_completed("resp-smoke-draft"),
            ]),
            sse(vec![
                ev_response_created("resp-smoke-final"),
                ev_assistant_message("msg-smoke-final", "final after stop hook"),
                ev_completed("resp-smoke-final"),
            ]),
        ],
    )
    .await;

    let mut builder = test_codex()
        .with_pre_build_hook(|home| {
            if let Err(error) = write_operator_smoke_hook_pack(home) {
                panic!("failed to write operator smoke hook pack: {error}");
            }
        })
        .with_config(trust_discovered_hooks);
    let test = builder.build(&server).await?;

    let first_turn_events = submit_smoke_turn(
        &test,
        "run the operator hook smoke with allow and stop coverage",
        AskForApproval::OnRequest,
        PermissionProfile::Disabled,
    )
    .await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 5);
    assert!(
        requests[0]
            .message_input_texts("developer")
            .contains(&SESSION_CONTEXT_A.to_string()),
        "first model request should include quiet SessionStart context from the first handler",
    );
    assert!(
        requests[0]
            .message_input_texts("developer")
            .contains(&SESSION_CONTEXT_B.to_string()),
        "first model request should include quiet SessionStart context from the second handler",
    );
    assert!(
        requests[0]
            .message_input_texts("developer")
            .contains(&USER_PROMPT_CONTEXT.to_string()),
        "first model request should include quiet UserPromptSubmit context",
    );
    assert!(
        requests[2]
            .message_input_texts("developer")
            .contains(&POST_TOOL_CONTEXT.to_string()),
        "follow-up request should include suppressed PostToolUse model context",
    );
    assert_eq!(
        request_hook_prompt_texts(&requests[4]),
        vec![STOP_CONTINUATION_PROMPT.to_string()],
        "stop hook block should continue the turn with the hook-authored prompt",
    );

    let pre_block_output = requests[1]
        .function_call_output(pre_block_call_id)
        .get("output")
        .and_then(Value::as_str)
        .context("pre block output text")?
        .to_string();
    assert!(pre_block_output.contains(PRE_BLOCK_REASON));
    assert!(pre_block_output.contains(&pre_block_command));
    assert!(
        pre_block_marker.exists(),
        "PreToolUse block should prevent command execution"
    );
    assert!(
        !permission_allow_marker.exists(),
        "PermissionRequest allow should let the command remove its marker"
    );
    let permission_deny_output = requests[3]
        .function_call_output(permission_deny_call_id)
        .get("output")
        .and_then(Value::as_str)
        .context("permission deny output text")?
        .to_string();
    assert!(
        permission_deny_output.contains(PERMISSION_DENY_REASON),
        "denied PermissionRequest output should show operator-readable feedback",
    );
    assert!(
        permission_deny_marker.exists(),
        "PermissionRequest deny should prevent command execution"
    );

    let first_runs = completed_runs(&first_turn_events);
    assert_run_entries(
        first_run(&first_runs, HookEventName::SessionStart),
        HookRunStatus::Completed,
        &[],
    );
    assert_run_entries(
        last_run(&first_runs, HookEventName::SessionStart),
        HookRunStatus::Completed,
        &[],
    );
    assert_run_entries(
        first_run(&first_runs, HookEventName::UserPromptSubmit),
        HookRunStatus::Completed,
        &[],
    );
    let pre_runs = runs_for_event(&first_runs, HookEventName::PreToolUse);
    assert_eq!(pre_runs.len(), 3);
    assert_run_entries(
        pre_runs[0],
        HookRunStatus::Blocked,
        &[(HookOutputEntryKind::Feedback, PRE_BLOCK_REASON)],
    );
    assert_run_entries(pre_runs[1], HookRunStatus::Completed, &[]);
    assert_run_entries(pre_runs[2], HookRunStatus::Completed, &[]);
    assert_run_entries(
        first_run(&first_runs, HookEventName::PermissionRequest),
        HookRunStatus::Completed,
        &[],
    );
    assert_run_entries(
        last_run(&first_runs, HookEventName::PermissionRequest),
        HookRunStatus::Blocked,
        &[(HookOutputEntryKind::Feedback, PERMISSION_DENY_REASON)],
    );
    assert_run_entries(
        first_run(&first_runs, HookEventName::PostToolUse),
        HookRunStatus::Completed,
        &[],
    );
    let stop_runs = runs_for_event(&first_runs, HookEventName::Stop);
    assert_eq!(stop_runs.len(), 2);
    assert_run_entries(
        stop_runs[0],
        HookRunStatus::Blocked,
        &[(HookOutputEntryKind::Feedback, STOP_CONTINUATION_PROMPT)],
    );
    assert_run_entries(stop_runs[1], HookRunStatus::Completed, &[]);

    let log = read_smoke_hook_log(test.codex_home_path())?;
    let session_handlers = log
        .iter()
        .filter(|entry| entry["event"] == "SessionStart")
        .map(|entry| entry["handler"].as_str().expect("handler").to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        session_handlers,
        vec!["session-a".to_string(), "session-b".to_string()],
        "matching SessionStart hooks should execute in configured order",
    );

    Ok(())
}

async fn submit_smoke_turn(
    test: &TestCodex,
    prompt: &str,
    approval_policy: AskForApproval,
    permission_profile: PermissionProfile,
) -> Result<Vec<EventMsg>> {
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(permission_profile, test.config.cwd.as_path());
    let session_model = test.session_configured.model.clone();
    test.codex
        .submit(Op::UserTurn {
            environments: None,
            items: vec![UserInput::Text {
                text: prompt.to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: test.config.cwd.to_path_buf(),
            approval_policy,
            approvals_reviewer: None,
            sandbox_policy,
            permission_profile,
            model: session_model,
            effort: None,
            summary: None,
            service_tier: None,
            collaboration_mode: None,
            personality: None,
        })
        .await?;

    let mut events = Vec::new();
    let mut turn_id = None;
    loop {
        let event =
            wait_for_event_with_timeout(&test.codex, |_| true, Duration::from_secs(30)).await;
        if let EventMsg::TurnStarted(started) = &event {
            turn_id = Some(started.turn_id.clone());
        }
        let turn_complete = matches!(
            &event,
            EventMsg::TurnComplete(completed)
                if turn_id.as_deref() == Some(completed.turn_id.as_str())
        );
        events.push(event);
        if turn_complete {
            break;
        }
    }
    Ok(events)
}

fn write_operator_smoke_hook_pack(home: &Path) -> Result<()> {
    let script_path = home.join("operator_smoke_hook.py");
    let script = r#"import json
from pathlib import Path
import sys

HOOK_DIR = Path(__file__).parent
LOG_PATH = HOOK_DIR / "operator_smoke_hook_log.jsonl"
STOP_MARKER_PATH = HOOK_DIR / "operator_smoke_stop_blocked"

handler = sys.argv[1]
payload = json.load(sys.stdin)
tool_input = payload.get("tool_input") or {}
command = tool_input.get("command") or ""

with LOG_PATH.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({
        "event": payload.get("hook_event_name"),
        "handler": handler,
        "command": command,
        "stop_hook_active": payload.get("stop_hook_active"),
    }) + "\n")

def quiet_context(event_name, context):
    return {
        "suppressOutput": True,
        "systemMessage": "hidden " + event_name + " smoke UI text",
        "hookSpecificOutput": {
            "hookEventName": event_name,
            "additionalContext": context,
        },
    }

if handler == "session-a":
    print(json.dumps(quiet_context("SessionStart", "smoke-session-context-a")))
elif handler == "session-b":
    print(json.dumps(quiet_context("SessionStart", "smoke-session-context-b")))
elif handler == "user-prompt":
    print(json.dumps(quiet_context("UserPromptSubmit", "smoke-user-prompt-context")))
elif handler == "pre":
    if "SMOKE_PRE_BLOCK" in command:
        print(json.dumps({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "smoke pre hook blocked this command",
            },
        }))
elif handler == "permission":
    if "SMOKE_PERMISSION_DENY" in command:
        print(json.dumps({
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {
                    "behavior": "deny",
                    "message": "smoke permission hook denied this command",
                },
            },
        }))
    elif "SMOKE_PERMISSION_ALLOW" in command:
        print(json.dumps({
            "hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {"behavior": "allow"},
            },
        }))
elif handler == "post":
    print(json.dumps(quiet_context("PostToolUse", "smoke-post-tool-context")))
elif handler == "stop":
    if not STOP_MARKER_PATH.exists() and not payload.get("stop_hook_active"):
        STOP_MARKER_PATH.write_text("blocked", encoding="utf-8")
        print(json.dumps({
            "decision": "block",
            "reason": "smoke stop hook continuation prompt",
        }))
"#;

    let command = |handler: &str| format!("python3 {} {handler}", script_path.display());
    let hooks = serde_json::json!({
        "hooks": {
            "SessionStart": [{
                "matcher": "startup",
                "hooks": [
                    {
                        "type": "command",
                        "command": command("session-a"),
                        "timeout": 10,
                        "statusMessage": "smoke session a",
                    },
                    {
                        "type": "command",
                        "command": command("session-b"),
                        "timeout": 10,
                        "statusMessage": "smoke session b",
                    },
                ],
            }],
            "UserPromptSubmit": [{
                "hooks": [{
                    "type": "command",
                    "command": command("user-prompt"),
                    "timeout": 10,
                    "statusMessage": "smoke user prompt",
                }],
            }],
            "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [{
                    "type": "command",
                    "command": command("pre"),
                    "timeout": 10,
                    "statusMessage": "smoke pre tool",
                }],
            }],
            "PermissionRequest": [{
                "matcher": "Bash",
                "hooks": [{
                    "type": "command",
                    "command": command("permission"),
                    "timeout": 10,
                    "statusMessage": "smoke permission",
                }],
            }],
            "PostToolUse": [{
                "matcher": "Bash",
                "hooks": [{
                    "type": "command",
                    "command": command("post"),
                    "timeout": 10,
                    "statusMessage": "smoke post tool",
                }],
            }],
            "Stop": [{
                "hooks": [{
                    "type": "command",
                    "command": command("stop"),
                    "timeout": 10,
                    "statusMessage": "smoke stop",
                }],
            }],
        },
    });

    fs::write(&script_path, script).context("write smoke hook script")?;
    fs::write(home.join("hooks.json"), hooks.to_string()).context("write smoke hooks.json")?;
    Ok(())
}

fn completed_runs(events: &[EventMsg]) -> Vec<&HookRunSummary> {
    events
        .iter()
        .filter_map(|event| match event {
            EventMsg::HookCompleted(completed) => Some(&completed.run),
            _ => None,
        })
        .collect()
}

fn runs_for_event<'a>(
    runs: &'a [&'a HookRunSummary],
    event_name: HookEventName,
) -> Vec<&'a HookRunSummary> {
    runs.iter()
        .copied()
        .filter(|run| run.event_name == event_name)
        .collect()
}

fn first_run<'a>(runs: &'a [&'a HookRunSummary], event_name: HookEventName) -> &'a HookRunSummary {
    runs.iter()
        .copied()
        .find(|run| run.event_name == event_name)
        .unwrap_or_else(|| panic!("missing completed {event_name:?} hook run"))
}

fn last_run<'a>(runs: &'a [&'a HookRunSummary], event_name: HookEventName) -> &'a HookRunSummary {
    runs.iter()
        .copied()
        .rev()
        .find(|run| run.event_name == event_name)
        .unwrap_or_else(|| panic!("missing completed {event_name:?} hook run"))
}

fn assert_run_entries(
    run: &HookRunSummary,
    status: HookRunStatus,
    expected_entries: &[(HookOutputEntryKind, &str)],
) {
    assert_eq!(run.status, status);
    let entries = run
        .entries
        .iter()
        .map(|entry| (entry.kind, entry.text.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(entries, expected_entries);
}

fn request_hook_prompt_texts(
    request: &core_test_support::responses::ResponsesRequest,
) -> Vec<String> {
    request
        .message_input_texts("user")
        .into_iter()
        .filter_map(|text| {
            codex_protocol::items::parse_hook_prompt_fragment(&text).map(|fragment| fragment.text)
        })
        .collect()
}

fn read_smoke_hook_log(home: &Path) -> Result<Vec<Value>> {
    fs::read_to_string(home.join("operator_smoke_hook_log.jsonl"))
        .context("read smoke hook log")?
        .lines()
        .map(|line| serde_json::from_str(line).context("parse smoke hook log line"))
        .collect()
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}
