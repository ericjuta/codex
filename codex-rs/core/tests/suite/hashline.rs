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
use xxhash_rust::xxh3::xxh3_64;
use xxhash_rust::xxh32::xxh32;

fn hashline_file_hash(contents: &str) -> String {
    format!("{:08x}", xxh3_64(contents.as_bytes()) >> 32)
}

fn hashline_line_hash(line: &str) -> String {
    format!("{:04x}", xxh32(line.as_bytes(), 0) & 0xffff)
}

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

/// The read-only permission profile routes hashline file access through the
/// bwrap-based fs sandbox helper; hosts that restrict unprivileged user
/// namespaces (e.g. Ubuntu with kernel.apparmor_restrict_unprivileged_userns=1)
/// cannot run it, so tests that need it should skip there.
#[cfg(target_os = "linux")]
fn fs_sandbox_helper_available() -> bool {
    std::process::Command::new("bwrap")
        .args([
            "--unshare-user",
            "--unshare-pid",
            "--unshare-net",
            "--ro-bind",
            "/",
            "/",
            "true",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn fs_sandbox_helper_available() -> bool {
    true
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
    fs::write(&file_path, "alpha\r\nbeta\r\ngamma\r\n")?;
    let bounded_file_name = "hashline-long-line.txt";
    fs::write(
        test.cwd.path().join(bounded_file_name),
        "\u{1f642}".repeat(100_000),
    )?;

    let read_args = json!({
        "path": file_name,
        "start_line": 1,
        "end_line": 2
    });
    let bounded_read_args = json!({
        "path": bounded_file_name,
    });
    let patch_args = json!({
        "path": file_name,
        "patch": format!(
            "[{file_name}]#{}\nSWAP 2:f589|bravo",
            hashline_file_hash("alpha\nbeta\ngamma\n")
        )
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
                "hashline-bounded-read-call",
                "hashline",
                "read",
                &serde_json::to_string(&bounded_read_args)?,
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

    assert_eq!(
        fs::read_to_string(file_path)?,
        "alpha\r\nbravo\r\ngamma\r\n"
    );

    let request = final_mock.single_request();
    let read_output = request
        .function_call_output_text("hashline-read-call")
        .expect("read output should be sent to model");
    assert!(read_output.contains("[hashline-notes.txt]#"));
    assert!(read_output.contains("2:"));
    assert!(read_output.contains("|beta"));
    let read_output: Value = serde_json::from_str(&read_output)?;
    assert_eq!(read_output["truncated"], json!(false));
    assert_eq!(read_output["next_start_line"], Value::Null);

    let bounded_read_output = request
        .function_call_output_text("hashline-bounded-read-call")
        .expect("bounded read output should be sent to model");
    assert!(bounded_read_output.len() <= 25 * 1024);
    let bounded_read_output: Value = serde_json::from_str(&bounded_read_output)?;
    assert_eq!(bounded_read_output["truncated"], json!(true));
    assert_eq!(
        bounded_read_output["lines"][0]["content_truncated"],
        json!(true)
    );

    let patch_output = request
        .function_call_output_text("hashline-patch-call")
        .expect("patch output should be sent to model");
    assert!(patch_output.contains("\"success\":true"));
    assert!(patch_output.contains(&format!("\"header\":\"[{file_name}]#")));
    assert!(patch_output.contains("\"operation\":\"update\""));
    assert!(patch_output.contains("|bravo"));
    assert!(!patch_output.contains("\\r"));
    assert!(patch_output.contains("\"preview\""));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_abort_marker_does_not_write() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-abort.txt";
    let file_path = test.cwd.path().join(file_name);
    fs::write(&file_path, "alpha\nbeta\n")?;

    let call_id = "hashline-abort-call";
    let patch_args = json!({
        "path": file_name,
        "patch": "*** Begin Patch\nSWAP 2:f589\n+bravo\n*** Abort\n*** End Patch"
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

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline patch aborted"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "abort the hashline patch").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(file_path)?, "alpha\nbeta\n");
    let request = final_mock.single_request();
    let patch_output = request
        .function_call_output_text(call_id)
        .expect("patch output should be sent to model");
    assert!(patch_output.contains("\"success\":true"));
    assert!(patch_output.contains("\"operation\":\"abort\""));
    assert!(patch_output.contains("\"aborted\":true"));
    assert!(patch_output.contains("\"dry_run\":false"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_find_block_reports_language_and_excerpt() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let dir_path = test.cwd.path().join("src");
    fs::create_dir_all(&dir_path)?;
    let file_name = "src/main.rs";
    let file_path = test.cwd.path().join(file_name);
    fs::write(
        &file_path,
        "fn main() {\r\n    if true {\r\n        println!(\"hi\");\r\n    }\r\n}\r\n",
    )?;

    let call_id = "hashline-find-block-call";
    let find_args = json!({
        "path": file_name,
        "anchor": "3"
    });
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call_with_namespace(
                call_id,
                "hashline",
                "find_block",
                &serde_json::to_string(&find_args)?,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline block found"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "find the Rust block with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let request = final_mock.single_request();
    let find_output = request
        .function_call_output_text(call_id)
        .expect("find_block output should be sent to model");
    let find_output_json: Value = serde_json::from_str(&find_output)?;
    assert!(find_output.contains("\"language\":\"Rust\""));
    assert!(find_output.contains("\"start_line\":2"));
    assert!(find_output.contains("\"end_line\":4"));
    assert!(find_output.contains("3:"));
    assert!(find_output.contains("println!"));
    assert_eq!(find_output_json["file"], json!(file_name));
    assert_eq!(find_output_json["line_count"], json!(5));
    assert_eq!(find_output_json["block_lines"][1]["n"], json!(3));
    assert_eq!(
        find_output_json["block_lines"][1]["hash"],
        json!(hashline_line_hash("        println!(\"hi\");")),
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_rejects_stale_line_hash() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-stale.txt";
    let file_path = test.cwd.path().join(file_name);
    fs::write(&file_path, "alpha\nbeta\ngamma\n")?;

    let call_id = "hashline-stale-call";
    let patch_args = json!({
        "path": file_name,
        "patch": format!(
            "[{file_name}]#{}\nSWAP 2:0000|bravo",
            hashline_file_hash("alpha\nbeta\ngamma\n")
        )
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

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline stale patch rejected"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "patch with a stale hashline anchor").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(file_path)?, "alpha\nbeta\ngamma\n");
    let request = final_mock.single_request();
    let patch_output = request
        .function_call_output_text(call_id)
        .expect("patch output should be sent to model");
    assert!(patch_output.contains("line 2 hash mismatch"));
    assert!(patch_output.contains("expected 0000"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_rejects_stale_hash_anchor_for_ins_post() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-stale-ins-post.txt";
    let file_path = test.cwd.path().join(file_name);
    fs::write(&file_path, "alpha\nbeta\ngamma\n")?;

    let call_id = "hashline-stale-ins-post-call";
    let stale_hash = if hashline_line_hash("beta") == "0000" {
        "0001"
    } else {
        "0000"
    };
    let patch_args = json!({
        "path": file_name,
        "patch": format!(
            "[{file_name}]#{}\nINS.POST 2:{stale_hash}|omega",
            hashline_file_hash("alpha\nbeta\ngamma\n")
        )
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

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline stale anchor rejected"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "patch with an invalid hash anchor").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(file_path)?, "alpha\nbeta\ngamma\n");
    let request = final_mock.single_request();
    let patch_output = request
        .function_call_output_text(call_id)
        .expect("patch output should be sent to model");

    assert!(patch_output.contains("line 2 hash mismatch"));
    assert!(patch_output.contains(&format!("expected {stale_hash}")));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_applies_multi_file_sections_through_apply_patch() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let first_name = "hashline-multi-a.txt";
    let second_name = "hashline-multi-b.txt";
    let first_path = test.cwd.path().join(first_name);
    let second_path = test.cwd.path().join(second_name);
    let first_contents = "alpha\nbeta\n";
    let second_contents = "one\ntwo\n";
    fs::write(&first_path, first_contents)?;
    fs::write(&second_path, second_contents)?;
    let first_hash = hashline_file_hash(first_contents);
    let second_hash = hashline_file_hash(second_contents);

    let call_id = "hashline-multi-file-call";
    let patch_args = json!({
        "path": first_name,
        "patch": format!(
            "[{first_name}]#{first_hash}\nSWAP 2:f589\n+bravo\n[{second_name}]#{second_hash}\nINS.TAIL:\n+three"
        )
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

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline multi-file patch complete"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "patch both files with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(first_path)?, "alpha\nbravo\n");
    assert_eq!(fs::read_to_string(second_path)?, "one\ntwo\nthree\n");
    let request = final_mock.single_request();
    let patch_output = request
        .function_call_output_text(call_id)
        .expect("patch output should be sent to model");
    assert!(patch_output.contains("\"success\":true"));
    assert!(patch_output.contains("\"operation\":\"multi_file_update\""));
    assert!(patch_output.contains(&format!("\"path\":\"{first_name}\"")));
    assert!(patch_output.contains(&format!("\"path\":\"{second_name}\"")));
    assert!(patch_output.contains(&format!("\"header\":\"[{first_name}]#")));
    assert!(patch_output.contains(&format!("\"header\":\"[{second_name}]#")));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_applies_mixed_multi_file_sections_through_apply_patch() -> anyhow::Result<()>
{
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let update_name = "hashline-mixed-update.txt";
    let remove_name = "hashline-mixed-remove.txt";
    let move_name = "hashline-mixed-move.txt";
    let moved_name = "hashline-mixed-moved.txt";
    let update_path = test.cwd.path().join(update_name);
    let remove_path = test.cwd.path().join(remove_name);
    let move_path = test.cwd.path().join(move_name);
    let moved_path = test.cwd.path().join(moved_name);
    let update_contents = "alpha\nbeta\n";
    let remove_contents = "remove me\n";
    let move_contents = "move me\nbefore\n";
    fs::write(&update_path, update_contents)?;
    fs::write(&remove_path, remove_contents)?;
    fs::write(&move_path, move_contents)?;
    let update_hash = hashline_file_hash(update_contents);
    let remove_hash = hashline_file_hash(remove_contents);
    let move_hash = hashline_file_hash(move_contents);

    let call_id = "hashline-mixed-multi-file-call";
    let patch_args = json!({
        "path": update_name,
        "patch": format!(
            "[{update_name}]#{update_hash}\nSWAP 2:f589\n+bravo\n[{remove_name}]#{remove_hash}\nREM\n[{move_name}]#{move_hash}\nMV {moved_name}\nSWAP 2:{}\n+after",
            hashline_line_hash("before")
        )
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

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline mixed multi-file patch complete"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "patch, remove, and move files with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(update_path)?, "alpha\nbravo\n");
    assert!(!remove_path.exists());
    assert!(!move_path.exists());
    assert_eq!(fs::read_to_string(moved_path)?, "move me\nafter\n");
    let request = final_mock.single_request();
    let patch_output = request
        .function_call_output_text(call_id)
        .expect("patch output should be sent to model");
    assert!(patch_output.contains("\"success\":true"));
    assert!(patch_output.contains("\"operation\":\"multi_file_operation\""));
    assert!(patch_output.contains(&format!("\"path\":\"{update_name}\"")));
    assert!(patch_output.contains(&format!("\"path\":\"{remove_name}\"")));
    assert!(patch_output.contains(&format!("\"path\":\"{move_name}\"")));
    assert!(patch_output.contains(&format!("\"new_path\":\"{moved_name}\"")));
    assert!(patch_output.contains("\"operation\":\"remove_file\""));
    assert!(patch_output.contains("\"operation\":\"rename_file\""));
    let patch_output_json: Value = serde_json::from_str(&patch_output)?;
    let files = patch_output_json["files"]
        .as_array()
        .expect("multi-file output should include files");
    let rename_file = files
        .iter()
        .find(|file| file["operation"] == json!("rename_file"))
        .expect("multi-file output should include a rename entry");
    assert_eq!(rename_file["src"], json!(move_name));
    assert_eq!(rename_file["dst"], json!(moved_name));
    assert_eq!(rename_file["operation"], json!("rename_file"));
    assert!(
        rename_file["header"]
            .as_str()
            .is_some_and(|header| header.starts_with(&format!("[{moved_name}]#")))
    );
    assert!(
        rename_file["content"]
            .as_str()
            .is_some_and(|content| content.contains("|after"))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_dry_run_outputs_report_success_without_writing() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let write_name = "hashline-dry-write.txt";
    let patch_name = "hashline-dry-patch.txt";
    let remove_name = "hashline-dry-remove.txt";
    let rename_name = "hashline-dry-rename.txt";
    let renamed_name = "hashline-dry-renamed.txt";
    let write_path = test.cwd.path().join(write_name);
    let patch_path = test.cwd.path().join(patch_name);
    let remove_path = test.cwd.path().join(remove_name);
    let rename_path = test.cwd.path().join(rename_name);
    let renamed_path = test.cwd.path().join(renamed_name);
    fs::write(&patch_path, "alpha\nbeta\n")?;
    fs::write(&remove_path, "remove me\n")?;
    fs::write(&rename_path, "move me\n")?;

    let write_call_id = "hashline-dry-write-call";
    let patch_call_id = "hashline-dry-patch-call";
    let remove_call_id = "hashline-dry-remove-call";
    let rename_call_id = "hashline-dry-rename-call";
    let write_args = json!({
        "path": write_name,
        "content": "created\n",
        "dry_run": true
    });
    let patch_args = json!({
        "path": patch_name,
        "patch": format!(
            "[{patch_name}]#{}\nSWAP 2:f589\n+bravo",
            hashline_file_hash("alpha\nbeta\n")
        ),
        "dry_run": true
    });
    let remove_args = json!({
        "path": remove_name,
        "expected_hash": hashline_file_hash("remove me\n"),
        "dry_run": true
    });
    let rename_args = json!({
        "path": rename_name,
        "new_path": renamed_name,
        "expected_hash": hashline_file_hash("move me\n"),
        "dry_run": true
    });
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call_with_namespace(
                write_call_id,
                "hashline",
                "write",
                &serde_json::to_string(&write_args)?,
            ),
            ev_function_call_with_namespace(
                patch_call_id,
                "hashline",
                "patch",
                &serde_json::to_string(&patch_args)?,
            ),
            ev_function_call_with_namespace(
                remove_call_id,
                "hashline",
                "remove_file",
                &serde_json::to_string(&remove_args)?,
            ),
            ev_function_call_with_namespace(
                rename_call_id,
                "hashline",
                "rename_file",
                &serde_json::to_string(&rename_args)?,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline dry runs checked"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "validate hashline dry runs").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert!(!write_path.exists());
    assert_eq!(fs::read_to_string(patch_path)?, "alpha\nbeta\n");
    assert_eq!(fs::read_to_string(&remove_path)?, "remove me\n");
    assert_eq!(fs::read_to_string(rename_path)?, "move me\n");
    assert!(!renamed_path.exists());

    let request = final_mock.single_request();
    let write_output = request
        .function_call_output_text(write_call_id)
        .expect("write output should be sent to model");
    let write_output_json: Value = serde_json::from_str(&write_output)?;
    assert_eq!(write_output_json["success"], json!(true));
    assert_eq!(write_output_json["dry_run"], json!(true));
    assert_eq!(write_output_json["operation"], json!("create"));

    let patch_output = request
        .function_call_output_text(patch_call_id)
        .expect("patch output should be sent to model");
    let patch_output_json: Value = serde_json::from_str(&patch_output)?;
    assert_eq!(patch_output_json["success"], json!(true));
    assert_eq!(patch_output_json["dry_run"], json!(true));
    assert_eq!(patch_output_json["operation"], json!("update"));
    assert!(patch_output_json["preview"].is_object());

    let remove_output = request
        .function_call_output_text(remove_call_id)
        .expect("remove output should be sent to model");
    let remove_output_json: Value = serde_json::from_str(&remove_output)?;
    assert_eq!(remove_output_json["success"], json!(true));
    assert_eq!(remove_output_json["dry_run"], json!(true));
    assert_eq!(remove_output_json["operation"], json!("remove_file"));

    let rename_output = request
        .function_call_output_text(rename_call_id)
        .expect("rename output should be sent to model");
    let rename_output_json: Value = serde_json::from_str(&rename_output)?;
    assert_eq!(rename_output_json["success"], json!(true));
    assert_eq!(rename_output_json["dry_run"], json!(true));
    assert_eq!(rename_output_json["operation"], json!("rename_file"));
    assert_eq!(rename_output_json["src"], json!(rename_name));
    assert_eq!(rename_output_json["dst"], json!(renamed_name));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_multi_file_dry_run_reports_success_per_file_without_writing() -> anyhow::Result<()>
{
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let update_name = "hashline-dry-multi-update.txt";
    let remove_name = "hashline-dry-multi-remove.txt";
    let move_name = "hashline-dry-multi-move.txt";
    let moved_name = "hashline-dry-multi-moved.txt";
    let update_path = test.cwd.path().join(update_name);
    let remove_path = test.cwd.path().join(remove_name);
    let move_path = test.cwd.path().join(move_name);
    let moved_path = test.cwd.path().join(moved_name);
    let update_contents = "alpha\nbeta\n";
    let remove_contents = "remove me\n";
    let move_contents = "move me\n";
    fs::write(&update_path, update_contents)?;
    fs::write(&remove_path, remove_contents)?;
    fs::write(&move_path, move_contents)?;
    let update_hash = hashline_file_hash(update_contents);
    let remove_hash = hashline_file_hash(remove_contents);
    let move_hash = hashline_file_hash(move_contents);

    let call_id = "hashline-dry-multi-call";
    let patch_args = json!({
        "path": update_name,
        "patch": format!(
            "[{update_name}]#{update_hash}\nSWAP 2:f589\n+bravo\n[{remove_name}]#{remove_hash}\nREM\n[{move_name}]#{move_hash}\nMV {moved_name}"
        ),
        "dry_run": true
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

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline multi-file dry run checked"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "validate multi-file hashline dry run").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(update_path)?, update_contents);
    assert_eq!(fs::read_to_string(remove_path)?, remove_contents);
    assert_eq!(fs::read_to_string(move_path)?, move_contents);
    assert!(!moved_path.exists());

    let request = final_mock.single_request();
    let patch_output = request
        .function_call_output_text(call_id)
        .expect("patch output should be sent to model");
    let patch_output_json: Value = serde_json::from_str(&patch_output)?;
    assert_eq!(patch_output_json["success"], json!(true));
    assert_eq!(patch_output_json["dry_run"], json!(true));
    assert_eq!(patch_output_json["total_files"], json!(3));
    assert_eq!(patch_output_json["files_truncated"], json!(false));
    assert_eq!(
        patch_output_json["operation"],
        json!("multi_file_operation")
    );

    let files = patch_output_json["files"]
        .as_array()
        .expect("multi-file output should include files");
    assert_eq!(files.len(), 3);
    for file in files {
        assert_eq!(file["success"], json!(true));
    }
    let update_file = files
        .iter()
        .find(|file| file["path"] == json!(update_name))
        .expect("multi-file output should include an update entry");
    assert_eq!(update_file["operation"], json!("update"));
    assert!(update_file["preview"].is_object());
    let remove_file = files
        .iter()
        .find(|file| file["operation"] == json!("remove_file"))
        .expect("multi-file output should include a remove entry");
    assert_eq!(remove_file["path"], json!(remove_name));
    let rename_file = files
        .iter()
        .find(|file| file["operation"] == json!("rename_file"))
        .expect("multi-file output should include a rename entry");
    assert_eq!(rename_file["src"], json!(move_name));
    assert_eq!(rename_file["dst"], json!(moved_name));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_can_create_missing_file() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-created.txt";
    let file_path = test.cwd.path().join(file_name);

    let call_id = "hashline-create-call";
    let patch_args = json!({
        "path": file_name,
        "patch": "INS.TAIL |created by hashline",
        "create": true
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

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline file created"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "create the file with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(file_path)?, "created by hashline");
    let request = final_mock.single_request();
    let patch_output = request
        .function_call_output_text(call_id)
        .expect("patch output should be sent to model");
    assert!(patch_output.contains("\"success\":true"));
    assert!(patch_output.contains(&format!("\"header\":\"[{file_name}]#")));
    assert!(patch_output.contains("\"operation\":\"create\""));
    assert!(patch_output.contains("1:"));
    assert!(patch_output.contains("|created by hashline"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_create_rejects_hashed_sections() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-created-hashed.txt";
    let file_path = test.cwd.path().join(file_name);

    let call_id = "hashline-create-hashed-call";
    let patch_args = json!({
        "path": file_name,
        "patch": format!("[{file_name}]#deadbeef\nINS.TAIL:\n+created by hashline"),
        "create": true
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

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline create rejected for hashed sections"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(
        &test,
        "create with a hashed section header should be rejected",
    )
    .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert!(!file_path.exists());
    let request = final_mock.single_request();
    let patch_output = request
        .function_call_output_text(call_id)
        .expect("patch output should be sent to model");
    assert!(patch_output.contains("create=true cannot use a [path]#HASH section header"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_can_create_multi_file_sections() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let first_name = "hashline-created-a.txt";
    let second_name = "hashline-created-b.txt";
    let first_path = test.cwd.path().join(first_name);
    let second_path = test.cwd.path().join(second_name);

    let call_id = "hashline-multi-create-call";
    let patch_args = json!({
        "path": first_name,
        "patch": format!(
            "[{first_name}]\n[{second_name}]\nINS.TAIL:\n+created beta"
        ),
        "create": true
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

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline files created"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "create both files with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert!(first_path.exists());
    assert_eq!(fs::metadata(&first_path)?.len(), 0);
    assert_eq!(fs::read_to_string(second_path)?, "created beta");
    let request = final_mock.single_request();
    let patch_output = request
        .function_call_output_text(call_id)
        .expect("patch output should be sent to model");
    assert!(patch_output.contains("\"success\":true"));
    assert!(patch_output.contains("\"operation\":\"multi_file_create\""));
    assert!(patch_output.contains(&format!("\"header\":\"[{first_name}]#")));
    assert!(patch_output.contains(&format!("\"header\":\"[{second_name}]#")));
    assert!(patch_output.contains("|created beta"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_can_create_missing_file_with_repeated_section_path() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-created-repeated.txt";
    let file_path = test.cwd.path().join(file_name);

    let call_id = "hashline-repeat-create-call";
    let patch_args = json!({
        "path": file_name,
        "patch": format!(
            "[{file_name}]\nINS.TAIL:\n+first\n[{file_name}]\nINS.TAIL:\n+second"
        ),
        "create": true
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

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline file created through repeated sections"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "create a file with repeated section headers").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(file_path)?, "first\nsecond");
    let request = final_mock.single_request();
    let patch_output = request
        .function_call_output_text(call_id)
        .expect("patch output should be sent to model");
    assert!(patch_output.contains("\"operation\":\"create\""));
    assert!(patch_output.contains(&format!("\"path\":\"{file_name}\"")));
    assert!(!patch_output.contains("\"operation\":\"multi_file_create\""));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_write_creates_empty_file_through_apply_patch() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-write-empty.txt";
    let file_path = test.cwd.path().join(file_name);

    let call_id = "hashline-write-empty-call";
    let write_args = json!({
        "path": file_name,
        "content": ""
    });
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call_with_namespace(
                call_id,
                "hashline",
                "write",
                &serde_json::to_string(&write_args)?,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline empty file written"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "write an empty file with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert!(file_path.exists());
    assert_eq!(fs::metadata(&file_path)?.len(), 0);
    let request = final_mock.single_request();
    let write_output = request
        .function_call_output_text(call_id)
        .expect("write output should be sent to model");
    assert!(write_output.contains("\"success\":true"));
    assert!(write_output.contains(&format!("\"header\":\"[{file_name}]#")));
    assert!(write_output.contains("\"operation\":\"create\""));
    assert!(write_output.contains("\"start_line\":null"));
    assert!(write_output.contains("\"end_line\":null"));
    assert!(write_output.contains("\"total_lines\":0"));
    assert!(write_output.contains("\"content\":\"\""));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_create_rejects_existing_file() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-existing.txt";
    let file_path = test.cwd.path().join(file_name);
    fs::write(&file_path, "keep me\n")?;

    let call_id = "hashline-create-existing-call";
    let patch_args = json!({
        "path": file_name,
        "patch": "INS.TAIL |do not overwrite",
        "create": true
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

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline create rejected"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "try to create an existing file with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(file_path)?, "keep me\n");
    let request = final_mock.single_request();
    let patch_output = request
        .function_call_output_text(call_id)
        .expect("patch output should be sent to model");
    assert!(patch_output.contains("Hashline operation requires"));
    assert!(patch_output.contains("already exists"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_write_creates_missing_file_through_apply_patch() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-write-created.txt";
    let file_path = test.cwd.path().join(file_name);

    let call_id = "hashline-write-create-call";
    let write_args = json!({
        "path": file_name,
        "content": "\u{feff}alpha\r\nbeta"
    });
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call_with_namespace(
                call_id,
                "hashline",
                "write",
                &serde_json::to_string(&write_args)?,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline file written"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "write the file with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(file_path)?, "alpha\nbeta");
    let request = final_mock.single_request();
    let write_output = request
        .function_call_output_text(call_id)
        .expect("write output should be sent to model");
    assert!(write_output.contains("\"success\":true"));
    assert!(write_output.contains(&format!("\"header\":\"[{file_name}]#")));
    assert!(write_output.contains("\"operation\":\"create\""));
    assert!(write_output.contains("1:"));
    assert!(write_output.contains("|alpha"));
    assert!(write_output.contains("|beta"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_write_rejects_existing_file_without_force() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-write-existing.txt";
    let file_path = test.cwd.path().join(file_name);
    fs::write(&file_path, "keep me\n")?;

    let call_id = "hashline-write-existing-call";
    let write_args = json!({
        "path": file_name,
        "content": "replace me\n"
    });
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call_with_namespace(
                call_id,
                "hashline",
                "write",
                &serde_json::to_string(&write_args)?,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline write rejected"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "try to overwrite the file with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(file_path)?, "keep me\n");
    let request = final_mock.single_request();
    let write_output = request
        .function_call_output_text(call_id)
        .expect("write output should be sent to model");
    assert!(write_output.contains("hashline.write refuses to overwrite existing file"));
    assert!(write_output.contains("force=true"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_write_overwrites_existing_file_with_force() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;
    let file_name = "hashline-write-force.txt";
    let file_path = test.cwd.path().join(file_name);

    let old_contents = (1..=200)
        .map(|line| format!("line {line}"))
        .chain(["alpha".to_string()])
        .collect::<Vec<_>>()
        .join("\n");
    let new_contents = (1..=200)
        .map(|line| format!("line {line}"))
        .chain(["omega".to_string(), "theta".to_string()])
        .collect::<Vec<_>>()
        .join("\r\n");
    let expected_contents = new_contents.replace("\r\n", "\n");
    fs::write(&file_path, &old_contents)?;

    let call_id = "hashline-write-force-call";
    let write_args = json!({
        "path": file_name,
        "content": &new_contents,
        "force": true
    });
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call_with_namespace(
                call_id,
                "hashline",
                "write",
                &serde_json::to_string(&write_args)?,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline file overwritten"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "force overwrite the file with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read_to_string(file_path)?, expected_contents);
    let request = final_mock.single_request();
    let write_output = request
        .function_call_output_text(call_id)
        .expect("write output should be sent to model");
    let write_output_json: Value = serde_json::from_str(&write_output)?;
    assert_eq!(write_output_json["success"], json!(true));
    assert_eq!(write_output_json["operation"], json!("update"));
    assert_eq!(write_output_json["start_line"], json!(201));
    assert_eq!(write_output_json["end_line"], json!(202));
    assert!(!write_output.contains("|line 1"));
    assert!(write_output.contains("|omega"));
    assert!(write_output.contains("|theta"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_remove_file_deletes_through_apply_patch() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-remove.txt";
    let file_path = test.cwd.path().join(file_name);
    fs::write(&file_path, "remove me\n")?;

    let call_id = "hashline-remove-call";
    let remove_args = json!({
        "path": file_name,
        "expected_hash": hashline_file_hash("remove me\n"),
    });
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call_with_namespace(
                call_id,
                "hashline",
                "remove_file",
                &serde_json::to_string(&remove_args)?,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline file removed"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "remove the file with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert!(!file_path.exists());
    let request = final_mock.single_request();
    let remove_output = request
        .function_call_output_text(call_id)
        .expect("remove output should be sent to model");
    assert!(remove_output.contains("\"success\":true"));
    assert!(remove_output.contains("\"operation\":\"remove_file\""));
    assert!(remove_output.contains(&format!("\"path\":\"{file_name}\"")));
    assert!(remove_output.contains("\"old_hash\""));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_rename_file_moves_through_apply_patch() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let old_name = "hashline-old-name.txt";
    let new_name = "hashline-new-name.txt";
    let old_path = test.cwd.path().join(old_name);
    let new_path = test.cwd.path().join(new_name);
    fs::write(&old_path, "first\nsecond")?;

    let call_id = "hashline-rename-call";
    let rename_args = json!({
        "path": old_name,
        "new_path": new_name,
        "expected_hash": hashline_file_hash("first\nsecond"),
    });
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call_with_namespace(
                call_id,
                "hashline",
                "rename_file",
                &serde_json::to_string(&rename_args)?,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline file renamed"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "rename the file with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert!(!old_path.exists());
    assert_eq!(fs::read_to_string(new_path)?, "first\nsecond");
    let request = final_mock.single_request();
    let rename_output = request
        .function_call_output_text(call_id)
        .expect("rename output should be sent to model");
    assert!(rename_output.contains("\"success\":true"));
    assert!(rename_output.contains("\"operation\":\"rename_file\""));
    assert!(rename_output.contains(&format!("\"path\":\"{old_name}\"")));
    assert!(rename_output.contains(&format!("\"new_path\":\"{new_name}\"")));
    assert!(rename_output.contains(&format!("\"header\":\"[{new_name}]#")));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_rename_file_moves_empty_file_through_apply_patch() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let old_name = "hashline-empty-old-name.txt";
    let new_name = "hashline-empty-new-name.txt";
    let old_path = test.cwd.path().join(old_name);
    let new_path = test.cwd.path().join(new_name);
    fs::write(&old_path, "")?;

    let call_id = "hashline-empty-rename-call";
    let rename_args = json!({
        "path": old_name,
        "new_path": new_name,
        "expected_hash": hashline_file_hash(""),
    });
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call_with_namespace(
                call_id,
                "hashline",
                "rename_file",
                &serde_json::to_string(&rename_args)?,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let final_mock = mount_sse_once(
        &server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline empty file renamed"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "rename the empty file with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert!(!old_path.exists());
    assert_eq!(fs::metadata(new_path)?.len(), 0);
    let request = final_mock.single_request();
    let rename_output = request
        .function_call_output_text(call_id)
        .expect("rename output should be sent to model");
    assert!(rename_output.contains("\"success\":true"));
    assert!(rename_output.contains("\"operation\":\"rename_file\""));
    assert!(rename_output.contains(&format!("\"header\":\"[{new_name}]#")));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_uses_apply_patch_approval_flow() -> anyhow::Result<()> {
    if !fs_sandbox_helper_available() {
        eprintln!(
            "skipping test: bwrap cannot create sandbox namespaces in this environment, so the read-only fs sandbox helper cannot run"
        );
        return Ok(());
    }

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
        "patch": format!(
            "[{file_name}]#{}\nSWAP 2:f589|bravo",
            hashline_file_hash("alpha\nbeta\ngamma\n")
        )
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
    let second_mock = mount_sse_once(
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
        let request = second_mock.single_request();
        panic!(
            "expected apply_patch approval request before completion; model saw: {:?}",
            request.function_call_output_text(call_id)
        );
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
    assert!(namespace_child_tool(&body, "hashline", "remove_file").is_some());
    assert!(namespace_child_tool(&body, "hashline", "rename_file").is_some());
    assert!(namespace_child_tool(&body, "hashline", "write").is_some());
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
