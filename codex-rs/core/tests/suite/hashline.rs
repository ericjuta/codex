use codex_protocol::models::PermissionProfile;
use codex_protocol::openai_models::ApplyPatchToolType;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::user_input::UserInput;
use core_test_support::TempDirExt;
use core_test_support::responses::ev_apply_patch_custom_tool_call;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call_with_namespace;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::namespace_child_tool;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::local_selections;
use core_test_support::test_codex::test_codex;
use core_test_support::test_codex::turn_permission_fields;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::fs;

async fn submit_turn(test: &TestCodex, prompt: &str) -> anyhow::Result<()> {
    let cwd = test.cwd.abs();
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd.as_path());

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: prompt.into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: codex_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(cwd)),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(codex_protocol::config_types::CollaborationMode {
                    mode: codex_protocol::config_types::ModeKind::Default,
                    settings: codex_protocol::config_types::Settings {
                        model: test.session_configured.model.clone(),
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await?;
    Ok(())
}

async fn submit_turn_with_read_only_approval(test: &TestCodex, prompt: &str) -> anyhow::Result<()> {
    let cwd = test.cwd.abs();
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::read_only(), cwd.as_path());

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: prompt.into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: codex_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(cwd)),
                approval_policy: Some(AskForApproval::OnRequest),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(codex_protocol::config_types::CollaborationMode {
                    mode: codex_protocol::config_types::ModeKind::Default,
                    settings: codex_protocol::config_types::Settings {
                        model: test.session_configured.model.clone(),
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_read_and_patch_tools_execute() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-notes.txt";
    let file_path = test.cwd.path().join(file_name);
    fs::write(&file_path, "alpha\nbeta\ngamma\n")?;

    let read_args = json!({
        "path": file_name,
        "start_line": 1,
        "end_line": 3
    });
    let patch_args = json!({
        "path": file_name,
        "patch": "SWAP 2|bravo"
    });
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call_with_namespace(
                "hashline-read-call",
                "hashline",
                "read",
                &serde_json::to_string(&read_args)?,
            ),
            ev_function_call_with_namespace(
                "hashline-patch-call",
                "hashline",
                "patch",
                &serde_json::to_string(&patch_args)?,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline edit complete"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "read and update the file with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(file_path)?, "alpha\nbravo\ngamma\n");

    let request = final_mock.single_request();
    let read_output = request
        .function_call_output_text("hashline-read-call")
        .expect("read output should be sent to model");
    assert!(read_output.contains("[hashline-notes.txt#"));
    assert!(read_output.contains("2:"));
    assert!(read_output.contains("|beta"));

    let patch_output = request
        .function_call_output_text("hashline-patch-call")
        .expect("patch output should be sent to model");
    assert!(patch_output.contains("Success. Updated the following files:"));
    assert!(patch_output.contains(file_name));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_uses_apply_patch_approval_flow() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-approval.txt";
    let file_path = test.cwd.path().join(file_name);
    fs::write(&file_path, "alpha\nbeta\ngamma\n")?;

    let call_id = "hashline-approval-call";
    let patch_args = json!({
        "path": file_name,
        "patch": "SWAP 2|bravo"
    });
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call_with_namespace(
                call_id,
                "hashline",
                "patch",
                &serde_json::to_string(&patch_args)?,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline edit approved"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn_with_read_only_approval(&test, "patch a read-only file with hashline").await?;

    let approval_event = wait_for_event(&test.codex, |event| {
        matches!(
            event,
            EventMsg::ApplyPatchApprovalRequest(_) | EventMsg::TurnComplete(_)
        )
    })
    .await;
    let EventMsg::ApplyPatchApprovalRequest(approval) = approval_event else {
        panic!("expected apply_patch approval request before completion");
    };
    assert_eq!(approval.call_id, call_id);
    assert!(
        approval.changes.keys().any(|path| path == &file_path),
        "approval should describe the file changed by hashline.patch"
    );

    test.codex
        .submit(Op::PatchApproval {
            id: approval.call_id,
            decision: ReviewDecision::Approved,
        })
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(&file_path)?, "alpha\nbravo\ngamma\n");
    let _ = fs::remove_file(&file_path);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_only_hides_apply_patch_from_model_visible_tools() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
            config.hashline.only = true;
        })
        .with_model("gpt-5.2")
        .with_model_info_override("gpt-5.2", |model_info| {
            model_info.apply_patch_tool_type = Some(ApplyPatchToolType::Freeform);
        })
        .build(&server)
        .await?;

    let first_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("msg-1", "no edits needed"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    submit_turn(&test, "inspect available edit tools").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let request = first_mock.single_request();
    let body = request.body_json();
    assert!(namespace_child_tool(&body, "hashline", "read").is_some());
    assert!(namespace_child_tool(&body, "hashline", "patch").is_some());
    assert!(namespace_child_tool(&body, "hashline", "find_block").is_some());
    assert!(!request_tools_include(&body, "apply_patch"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_only_keeps_apply_patch_dispatch_for_compatibility() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
            config.hashline.only = true;
        })
        .with_model("gpt-5.2")
        .with_model_info_override("gpt-5.2", |model_info| {
            model_info.apply_patch_tool_type = Some(ApplyPatchToolType::Freeform);
        })
        .build(&server)
        .await?;

    let file_name = "legacy-apply-patch.txt";
    let file_path = test.cwd.path().join(file_name);
    let patch =
        format!("*** Begin Patch\n*** Add File: {file_name}\n+legacy compatibility\n*** End Patch");
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_apply_patch_custom_tool_call("legacy-apply-patch-call", &patch),
            ev_completed("resp-1"),
        ]),
    )
    .await;
    mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "legacy patch complete"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "replay an old apply_patch call").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(file_path)?, "legacy compatibility\n");
    Ok(())
}

fn request_tools_include(body: &Value, tool_name: &str) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| tool.get("name").and_then(Value::as_str) == Some(tool_name))
        })
}
