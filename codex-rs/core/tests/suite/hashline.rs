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
use core_test_support::test_codex::TestCodexHarness;
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

async fn run_hashline_tool(
    harness: &TestCodexHarness,
    call_id: &str,
    tool_name: &str,
    arguments: &Value,
    prompt: &str,
) -> anyhow::Result<String> {
    let response_id = format!("{call_id}-response");
    mount_sse_once(
        harness.server(),
        sse(vec![
            ev_response_created(&response_id),
            ev_function_call_with_namespace(
                call_id,
                "hashline",
                tool_name,
                &serde_json::to_string(arguments)?,
            ),
            ev_completed(&response_id),
        ]),
    )
    .await;

    let final_response_id = format!("{call_id}-final-response");
    let final_mock = mount_sse_once(
        harness.server(),
        sse(vec![
            ev_assistant_message(&format!("{call_id}-message"), "hashline test call complete"),
            ev_completed(&final_response_id),
        ]),
    )
    .await;

    harness.submit(prompt).await?;
    Ok(final_mock
        .single_request()
        .function_call_output_text(call_id)
        .expect("hashline output should be sent to model"))
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
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.hashline.enabled = true;
    }))
    .await?;
    let server = harness.server();

    let file_name = "hashline-notes.txt";
    harness
        .write_file(file_name, "alpha\r\nbeta\r\ngamma\r\n")
        .await?;
    let bounded_file_name = "hashline-long-line.txt";
    harness
        .write_file(bounded_file_name, "\u{1f642}".repeat(100_000))
        .await?;

    let read_args = json!({
        "path": file_name,
        "start_line": 1,
        "end_line": 2
    });
    let bounded_read_args = json!({
        "path": bounded_file_name,
    });
    mount_sse_once(
        server,
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
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let read_result_mock = mount_sse_once(
        server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline read complete"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    harness.submit("read the file with hashline").await?;

    let request = read_result_mock.single_request();
    let read_output = request
        .function_call_output_text("hashline-read-call")
        .expect("read output should be sent to model");
    let read_output: Value = serde_json::from_str(&read_output)?;
    let file_hash = hashline_file_hash("alpha\nbeta\ngamma\n");
    let alpha_hash = hashline_line_hash("alpha");
    let beta_hash = hashline_line_hash("beta");
    assert_eq!(
        read_output,
        json!({
            "path": file_name,
            "hash": file_hash,
            "header": format!("[{file_name}]#{file_hash}"),
            "start_line": 1,
            "end_line": 2,
            "total_lines": 3,
            "truncated": false,
            "next_start_line": null,
            "content": format!("1:{alpha_hash}|alpha\n2:{beta_hash}|beta"),
            "lines": [
                {"n": 1, "hash": alpha_hash},
                {"n": 2, "hash": beta_hash},
            ],
        })
    );

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

    let patch_args = json!({
        "path": file_name,
        "patch": format!(
            "{}\nSWAP {}:{}|bravo",
            read_output["header"].as_str().expect("read header should be a string"),
            read_output["lines"][1]["n"]
                .as_u64()
                .expect("read line number should be an integer"),
            read_output["lines"][1]["hash"]
                .as_str()
                .expect("read line hash should be a string"),
        ),
    });
    mount_sse_once(
        server,
        sse(vec![
            ev_response_created("resp-3"),
            ev_function_call_with_namespace(
                "hashline-patch-call",
                "hashline",
                "patch",
                &serde_json::to_string(&patch_args)?,
            ),
            ev_completed("resp-3"),
        ]),
    )
    .await;
    let patch_result_mock = mount_sse_once(
        server,
        sse(vec![
            ev_assistant_message("msg-2", "hashline edit complete"),
            ev_completed("resp-4"),
        ]),
    )
    .await;

    harness
        .submit("update the line using the hashline.read anchors")
        .await?;

    assert_eq!(
        harness.read_file_text(file_name).await?,
        "alpha\r\nbravo\r\ngamma\r\n"
    );

    let patch_output = patch_result_mock
        .single_request()
        .function_call_output_text("hashline-patch-call")
        .expect("patch output should be sent to model");
    let patch_output: Value = serde_json::from_str(&patch_output)?;
    let new_hash = hashline_file_hash("alpha\nbravo\ngamma\n");
    let bravo_hash = hashline_line_hash("bravo");
    assert_eq!(
        patch_output,
        json!({
            "success": true,
            "path": file_name,
            "header": format!("[{file_name}]#{new_hash}"),
            "operation": "update",
            "old_hash": read_output["hash"],
            "new_hash": new_hash,
            "start_line": 2,
            "end_line": 2,
            "total_lines": 3,
            "truncated": false,
            "content": format!("2:{bravo_hash}|bravo"),
            "lines": [{"n": 2, "hash": bravo_hash}],
            "preview": {
                "old_start_line": 2,
                "old_end_line": 2,
                "new_start_line": 2,
                "new_end_line": 2,
                "truncated": false,
                "content": format!("-2:{beta_hash}|beta\n+2:{bravo_hash}|bravo"),
            },
        })
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_restores_no_final_newline_after_apply_patch() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let test = test_codex()
        .with_config(|config| {
            config.hashline.enabled = true;
        })
        .build(&server)
        .await?;

    let file_name = "hashline-no-final-newline.txt";
    let file_path = test.cwd.path().join(file_name);
    fs::write(&file_path, "alpha\r\nbeta")?;

    let call_id = "hashline-no-final-newline-call";
    let patch_args = json!({
        "path": file_name,
        "patch": format!(
            "[{file_name}]#{}\nSWAP 2:{}|bravo",
            hashline_file_hash("alpha\nbeta"),
            hashline_line_hash("beta")
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
            ev_assistant_message("msg-1", "hashline edit complete"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    submit_turn(&test, "update the no-final-newline file with hashline").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(fs::read(file_path)?, b"alpha\r\nbravo");
    let request = final_mock.single_request();
    let patch_output = request
        .function_call_output_text(call_id)
        .expect("patch output should be sent to model");
    let patch_output: Value = serde_json::from_str(&patch_output)?;
    assert_eq!(patch_output["success"], json!(true));
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
async fn hashline_find_block_output_round_trips_to_swap_block() -> anyhow::Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.hashline.enabled = true;
    }))
    .await?;
    let server = harness.server();

    let file_name = "src/main.rs";
    let original = "fn main() {\r\n    if true {\r\n        println!(\"hi\");\r\n    }\r\n}\r\n";
    harness.write_file(file_name, original).await?;

    let call_id = "hashline-find-block-call";
    let anchor = format!("3:{}", hashline_line_hash("        println!(\"hi\");"));
    let find_args = json!({
        "path": file_name,
        "anchor": anchor,
    });
    mount_sse_once(
        server,
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

    let find_result_mock = mount_sse_once(
        server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline block found"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    harness.submit("find the Rust block with hashline").await?;

    let request = find_result_mock.single_request();
    let find_output = request
        .function_call_output_text(call_id)
        .expect("find_block output should be sent to model");
    let find_output: Value = serde_json::from_str(&find_output)?;
    let normalized = "fn main() {\n    if true {\n        println!(\"hi\");\n    }\n}\n";
    let block = "    if true {\n        println!(\"hi\");\n    }";
    let block_hash = hashline_file_hash(block);
    let line_two_hash = hashline_line_hash("    if true {");
    let line_three_hash = hashline_line_hash("        println!(\"hi\");");
    let line_four_hash = hashline_line_hash("    }");
    let expected_block_anchor = format!("3:{line_three_hash}@{block_hash}");
    assert_eq!(
        find_output,
        json!({
            "file": file_name,
            "path": file_name,
            "hash": hashline_file_hash(normalized),
            "header": format!("[{file_name}]#{}", hashline_file_hash(normalized)),
            "block_hash": block_hash,
            "block_anchor": expected_block_anchor,
            "anchor": anchor,
            "line_count": 5,
            "language": "Rust",
            "start_line": 2,
            "end_line": 4,
            "truncated": false,
            "content": format!(
                "2:{line_two_hash}|    if true {{\n3:{line_three_hash}|        println!(\"hi\");\n4:{line_four_hash}|    }}"
            ),
            "block_lines": [
                {"n": 2, "hash": line_two_hash},
                {"n": 3, "hash": line_three_hash},
                {"n": 4, "hash": line_four_hash},
            ],
        })
    );

    let patch_args = json!({
        "path": file_name,
        "patch": format!(
            "{}\nSWAP.BLK {}:\n+    if false {{\n+        println!(\"bye\");\n+    }}",
            find_output["header"]
                .as_str()
                .expect("find_block header should be a string"),
            find_output["block_anchor"]
                .as_str()
                .expect("find_block anchor should be a string"),
        ),
    });
    mount_sse_once(
        server,
        sse(vec![
            ev_response_created("resp-3"),
            ev_function_call_with_namespace(
                "hashline-swap-block-call",
                "hashline",
                "patch",
                &serde_json::to_string(&patch_args)?,
            ),
            ev_completed("resp-3"),
        ]),
    )
    .await;
    let patch_result_mock = mount_sse_once(
        server,
        sse(vec![
            ev_assistant_message("msg-2", "hashline block updated"),
            ev_completed("resp-4"),
        ]),
    )
    .await;

    harness
        .submit("replace the block using the exact find_block fields")
        .await?;

    assert_eq!(
        harness.read_file_text(file_name).await?,
        "fn main() {\r\n    if false {\r\n        println!(\"bye\");\r\n    }\r\n}\r\n"
    );
    let patch_output = patch_result_mock
        .single_request()
        .function_call_output_text("hashline-swap-block-call")
        .expect("SWAP.BLK output should be sent to model");
    let patch_output: Value = serde_json::from_str(&patch_output)?;
    assert_eq!(patch_output["success"], json!(true));
    assert_eq!(patch_output["operation"], json!("update"));
    assert_eq!(patch_output["path"], json!(file_name));
    assert_eq!(patch_output["old_hash"], find_output["hash"]);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_swap_block_rejects_stale_block_without_writing() -> anyhow::Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.hashline.enabled = true;
    }))
    .await?;
    let server = harness.server();
    let file_name = "src/stale_block.rs";
    let original = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";
    let externally_mutated = "fn main() {\n    let x = 1;\n    dbg!(x);\n}\n";
    harness.write_file(file_name, original).await?;

    let find_call_id = "hashline-stale-block-find-call";
    let find_args = json!({
        "path": file_name,
        "anchor": format!("2:{}", hashline_line_hash("    let x = 1;")),
    });
    mount_sse_once(
        server,
        sse(vec![
            ev_response_created("resp-stale-block-1"),
            ev_function_call_with_namespace(
                find_call_id,
                "hashline",
                "find_block",
                &serde_json::to_string(&find_args)?,
            ),
            ev_completed("resp-stale-block-1"),
        ]),
    )
    .await;
    let find_result_mock = mount_sse_once(
        server,
        sse(vec![
            ev_assistant_message("msg-stale-block-1", "hashline block captured"),
            ev_completed("resp-stale-block-2"),
        ]),
    )
    .await;

    harness.submit("find the block before editing it").await?;
    let find_output = find_result_mock
        .single_request()
        .function_call_output_text(find_call_id)
        .expect("find_block output should be sent to model");
    let find_output: Value = serde_json::from_str(&find_output)?;
    assert_eq!(find_output["start_line"], json!(1));
    assert_eq!(find_output["end_line"], json!(4));

    harness.write_file(file_name, externally_mutated).await?;

    let patch_call_id = "hashline-stale-block-patch-call";
    let patch_args = json!({
        "path": file_name,
        "patch": format!(
            "[{file_name}]#{}\nSWAP.BLK {}:\n+fn replacement() {{}}",
            hashline_file_hash(externally_mutated),
            find_output["block_anchor"]
                .as_str()
                .expect("find_block anchor should be a string"),
        ),
    });
    mount_sse_once(
        server,
        sse(vec![
            ev_response_created("resp-stale-block-3"),
            ev_function_call_with_namespace(
                patch_call_id,
                "hashline",
                "patch",
                &serde_json::to_string(&patch_args)?,
            ),
            ev_completed("resp-stale-block-3"),
        ]),
    )
    .await;
    let patch_result_mock = mount_sse_once(
        server,
        sse(vec![
            ev_assistant_message("msg-stale-block-2", "stale block rejected"),
            ev_completed("resp-stale-block-4"),
        ]),
    )
    .await;

    harness
        .submit("replace the block using the previously returned block anchor")
        .await?;

    assert_eq!(harness.read_file_text(file_name).await?, externally_mutated);
    let patch_output = patch_result_mock
        .single_request()
        .function_call_output_text(patch_call_id)
        .expect("stale SWAP.BLK output should be sent to model");
    assert!(patch_output.contains("block hash mismatch"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_patch_rejects_file_mutation_after_read_without_writing() -> anyhow::Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.hashline.enabled = true;
    }))
    .await?;
    let server = harness.server();
    let file_name = "hashline-stale.txt";
    let original = "alpha\nbeta\ngamma\n";
    let externally_mutated = "alpha\nchanged elsewhere\ngamma\n";
    harness.write_file(file_name, original).await?;

    let read_call_id = "hashline-stale-read-call";
    let read_args = json!({
        "path": file_name,
        "start_line": 2,
        "end_line": 2,
    });
    mount_sse_once(
        server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call_with_namespace(
                read_call_id,
                "hashline",
                "read",
                &serde_json::to_string(&read_args)?,
            ),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let read_result_mock = mount_sse_once(
        server,
        sse(vec![
            ev_assistant_message("msg-1", "hashline anchors captured"),
            ev_completed("resp-2"),
        ]),
    )
    .await;

    harness.submit("read the file before editing it").await?;
    let read_output = read_result_mock
        .single_request()
        .function_call_output_text(read_call_id)
        .expect("read output should be sent to model");
    let read_output: Value = serde_json::from_str(&read_output)?;
    assert_eq!(read_output["hash"], json!(hashline_file_hash(original)));
    assert_eq!(
        read_output["lines"],
        json!([{"n": 2, "hash": hashline_line_hash("beta")}])
    );

    harness.write_file(file_name, externally_mutated).await?;

    let patch_call_id = "hashline-stale-patch-call";
    let patch_args = json!({
        "path": file_name,
        "patch": format!(
            "{}\nSWAP {}:{}|bravo",
            read_output["header"].as_str().expect("read header should be a string"),
            read_output["lines"][0]["n"]
                .as_u64()
                .expect("read line number should be an integer"),
            read_output["lines"][0]["hash"]
                .as_str()
                .expect("read line hash should be a string"),
        ),
    });
    mount_sse_once(
        server,
        sse(vec![
            ev_response_created("resp-3"),
            ev_function_call_with_namespace(
                patch_call_id,
                "hashline",
                "patch",
                &serde_json::to_string(&patch_args)?,
            ),
            ev_completed("resp-3"),
        ]),
    )
    .await;
    let patch_result_mock = mount_sse_once(
        server,
        sse(vec![
            ev_assistant_message("msg-2", "hashline stale patch rejected"),
            ev_completed("resp-4"),
        ]),
    )
    .await;

    harness
        .submit("apply the edit using the previously returned anchors")
        .await?;

    assert_eq!(harness.read_file_text(file_name).await?, externally_mutated);
    let patch_output = patch_result_mock
        .single_request()
        .function_call_output_text(patch_call_id)
        .expect("patch output should be sent to model");
    assert!(patch_output.contains("file hash mismatch"));
    assert!(
        patch_output.contains(
            read_output["hash"]
                .as_str()
                .expect("read hash should be a string")
        )
    );
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
            "[{first_name}]\nINS.TAIL:\n+created alpha\n[{second_name}]\nINS.TAIL:\n+created beta"
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

    assert_eq!(fs::read_to_string(first_path)?, "created alpha");
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
async fn hashline_remove_file_rejects_stale_read_hash_without_deleting() -> anyhow::Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.hashline.enabled = true;
    }))
    .await?;
    let server = harness.server();
    let file_name = "hashline-stale-remove.txt";
    let original = "remove me\n";
    let externally_mutated = "keep the newer contents\n";
    harness.write_file(file_name, original).await?;

    let read_call_id = "hashline-stale-remove-read-call";
    let read_args = json!({"path": file_name});
    mount_sse_once(
        server,
        sse(vec![
            ev_response_created("resp-stale-remove-1"),
            ev_function_call_with_namespace(
                read_call_id,
                "hashline",
                "read",
                &serde_json::to_string(&read_args)?,
            ),
            ev_completed("resp-stale-remove-1"),
        ]),
    )
    .await;
    let read_result_mock = mount_sse_once(
        server,
        sse(vec![
            ev_assistant_message("msg-stale-remove-1", "hashline remove guard captured"),
            ev_completed("resp-stale-remove-2"),
        ]),
    )
    .await;

    harness.submit("read the file before removing it").await?;
    let read_output = read_result_mock
        .single_request()
        .function_call_output_text(read_call_id)
        .expect("read output should be sent to model");
    let read_output: Value = serde_json::from_str(&read_output)?;
    assert_eq!(read_output["hash"], json!(hashline_file_hash(original)));

    harness.write_file(file_name, externally_mutated).await?;

    let remove_call_id = "hashline-stale-remove-call";
    let remove_args = json!({
        "path": file_name,
        "expected_hash": read_output["hash"],
    });
    mount_sse_once(
        server,
        sse(vec![
            ev_response_created("resp-stale-remove-3"),
            ev_function_call_with_namespace(
                remove_call_id,
                "hashline",
                "remove_file",
                &serde_json::to_string(&remove_args)?,
            ),
            ev_completed("resp-stale-remove-3"),
        ]),
    )
    .await;
    let remove_result_mock = mount_sse_once(
        server,
        sse(vec![
            ev_assistant_message("msg-stale-remove-2", "stale remove rejected"),
            ev_completed("resp-stale-remove-4"),
        ]),
    )
    .await;

    harness
        .submit("remove the file using the previously returned hash")
        .await?;

    assert!(harness.path_exists(file_name).await?);
    assert_eq!(harness.read_file_text(file_name).await?, externally_mutated);
    let remove_output = remove_result_mock
        .single_request()
        .function_call_output_text(remove_call_id)
        .expect("remove output should be sent to model");
    assert!(remove_output.contains("file hash mismatch"));

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_review_multi_file_stale_final_section_is_atomic() -> anyhow::Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.hashline.enabled = true;
    }))
    .await?;

    let update_name = "hashline-atomic-update.txt";
    let remove_name = "hashline-atomic-remove.txt";
    let rename_name = "hashline-atomic-rename.txt";
    let renamed_name = "hashline-atomic-renamed.txt";
    let update_contents = "alpha\nbeta\n";
    let remove_contents = "remove me\n";
    let rename_contents = "rename me\n";
    harness.write_file(update_name, update_contents).await?;
    harness.write_file(remove_name, remove_contents).await?;
    harness.write_file(rename_name, rename_contents).await?;

    let rename_hash = hashline_file_hash(rename_contents);
    let stale_rename_hash = if rename_hash == "00000000" {
        "00000001"
    } else {
        "00000000"
    };
    let patch_args = json!({
        "path": update_name,
        "patch": format!(
            "[{update_name}]#{}\nSWAP 2:{}\n+bravo\n[{remove_name}]#{}\nREM\n[{rename_name}]#{stale_rename_hash}\nMV {renamed_name}",
            hashline_file_hash(update_contents),
            hashline_line_hash("beta"),
            hashline_file_hash(remove_contents),
        ),
    });

    let output = run_hashline_tool(
        &harness,
        "hashline-atomic-stale-call",
        "patch",
        &patch_args,
        "try the multi-file patch with the stale final section",
    )
    .await?;

    assert!(output.contains("file hash mismatch"));
    assert_eq!(harness.read_file_text(update_name).await?, update_contents);
    assert_eq!(harness.read_file_text(remove_name).await?, remove_contents);
    assert_eq!(harness.read_file_text(rename_name).await?, rename_contents);
    assert!(!harness.path_exists(renamed_name).await?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_review_bom_mixed_endings_cardinality_edit_preserves_exact_bytes()
-> anyhow::Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.hashline.enabled = true;
    }))
    .await?;

    let file_name = "hashline-bom-mixed-endings.txt";
    let original = "\u{feff}alpha\r\nbeta\ngamma\rdelta\n";
    let normalized = "alpha\nbeta\ngamma\ndelta\n";
    let expected = "\u{feff}alpha\r\nbravo\ncharlie\ngamma\rdelta\n";
    harness.write_file(file_name, original).await?;
    let patch_args = json!({
        "path": file_name,
        "patch": format!(
            "[{file_name}]#{}\nSWAP 2:{}:\n+bravo\n+charlie",
            hashline_file_hash(normalized),
            hashline_line_hash("beta"),
        ),
    });

    let output = run_hashline_tool(
        &harness,
        "hashline-bom-mixed-call",
        "patch",
        &patch_args,
        "apply the mixed-ending cardinality edit",
    )
    .await?;

    assert_eq!(
        serde_json::from_str::<Value>(&output)?["success"],
        json!(true)
    );
    let actual = harness.read_file_text(file_name).await?;
    assert_eq!(actual.as_bytes(), expected.as_bytes());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_review_multi_file_detail_budget_bounds_dry_run_and_applied_output()
-> anyhow::Result<()> {
    const FILE_COUNT: usize = 32;

    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.hashline.enabled = true;
    }))
    .await?;

    let mut dry_patch = String::new();
    let mut applied_patch = String::new();
    let mut dry_files = Vec::new();
    let mut applied_files = Vec::new();
    for index in 0..FILE_COUNT {
        let dry_name = format!("hashline-budget-dry-{index}.txt");
        let applied_name = format!("hashline-budget-applied-{index}.txt");
        let old_contents = format!("old-{index}-{}\n", "x".repeat(6_000));
        let new_contents = format!("new-{index}-{}\n", "y".repeat(6_000));
        harness.write_file(&dry_name, &old_contents).await?;
        harness.write_file(&applied_name, &old_contents).await?;
        let section = |name: &str| {
            format!(
                "[{name}]#{}\nSWAP 1:{}:\n+{}",
                hashline_file_hash(&old_contents),
                hashline_line_hash(old_contents.trim_end()),
                new_contents.trim_end(),
            )
        };
        if index > 0 {
            dry_patch.push('\n');
            applied_patch.push('\n');
        }
        dry_patch.push_str(&section(&dry_name));
        applied_patch.push_str(&section(&applied_name));
        dry_files.push((dry_name, old_contents.clone()));
        applied_files.push((applied_name, new_contents));
    }

    let dry_output = run_hashline_tool(
        &harness,
        "hashline-budget-dry-call",
        "patch",
        &json!({
            "path": dry_files[0].0,
            "patch": dry_patch,
            "dry_run": true,
        }),
        "dry-run the oversized multi-file details",
    )
    .await?;
    let applied_output = run_hashline_tool(
        &harness,
        "hashline-budget-applied-call",
        "patch",
        &json!({
            "path": applied_files[0].0,
            "patch": applied_patch,
        }),
        "apply the oversized multi-file details",
    )
    .await?;

    for (output, dry_run) in [(&dry_output, true), (&applied_output, false)] {
        let body: Value = serde_json::from_str(output)?;
        let files = body["files"]
            .as_array()
            .expect("multi-file output should contain bounded details");
        assert_eq!(body["success"], json!(true));
        assert_eq!(body["total_files"], json!(FILE_COUNT));
        assert_eq!(
            body["files_truncated"],
            json!(true),
            "dry_run={dry_run}, returned {} of {FILE_COUNT} details",
            files.len()
        );
        assert!(!files.is_empty());
        assert!(files.len() < FILE_COUNT);
        assert!(serde_json::to_vec(&body["files"])?.len() <= 24 * 1024);
        assert!(
            output.len() <= 26 * 1024,
            "output was {} bytes",
            output.len()
        );
        if dry_run {
            assert_eq!(body["dry_run"], json!(true));
        } else {
            assert!(body.get("dry_run").is_none());
        }
    }
    for (name, original) in dry_files {
        assert_eq!(harness.read_file_text(name).await?, original);
    }
    for (name, expected) in applied_files {
        assert_eq!(harness.read_file_text(name).await?, expected);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_review_single_section_remove_rename_and_rename_with_edit() -> anyhow::Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.hashline.enabled = true;
    }))
    .await?;

    let remove_name = "hashline-single-remove.txt";
    let rename_name = "hashline-single-rename.txt";
    let renamed_name = "hashline-single-renamed.txt";
    let edit_name = "hashline-single-rename-edit.txt";
    let edited_name = "hashline-single-renamed-edited.txt";
    let remove_contents = "remove me\n";
    let rename_contents = "rename me\n";
    let edit_contents = "before\nchange me\n";
    harness.write_file(remove_name, remove_contents).await?;
    harness.write_file(rename_name, rename_contents).await?;
    harness.write_file(edit_name, edit_contents).await?;

    let remove_output = run_hashline_tool(
        &harness,
        "hashline-single-remove-call",
        "patch",
        &json!({
            "path": remove_name,
            "patch": format!("[{remove_name}]#{}\nREM", hashline_file_hash(remove_contents)),
        }),
        "remove one file through a single patch section",
    )
    .await?;
    let rename_output = run_hashline_tool(
        &harness,
        "hashline-single-rename-call",
        "patch",
        &json!({
            "path": rename_name,
            "patch": format!(
                "[{rename_name}]#{}\nMV {renamed_name}",
                hashline_file_hash(rename_contents),
            ),
        }),
        "rename one file through a single patch section",
    )
    .await?;
    let edit_output = run_hashline_tool(
        &harness,
        "hashline-single-rename-edit-call",
        "patch",
        &json!({
            "path": edit_name,
            "patch": format!(
                "[{edit_name}]#{}\nMV {edited_name}\nSWAP 2:{}\n+changed",
                hashline_file_hash(edit_contents),
                hashline_line_hash("change me"),
            ),
        }),
        "rename and edit one file through a single patch section",
    )
    .await?;

    assert!(!harness.path_exists(remove_name).await?);
    assert!(!harness.path_exists(rename_name).await?);
    assert_eq!(harness.read_file_text(renamed_name).await?, rename_contents);
    assert!(!harness.path_exists(edit_name).await?);
    assert_eq!(
        harness.read_file_text(edited_name).await?,
        "before\nchanged\n"
    );
    for output in [&remove_output, &rename_output, &edit_output] {
        assert_eq!(
            serde_json::from_str::<Value>(output)?["success"],
            json!(true)
        );
    }
    assert_eq!(
        serde_json::from_str::<Value>(&remove_output)?["operation"],
        json!("remove_file")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&rename_output)?["operation"],
        json!("rename_file")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&edit_output)?["operation"],
        json!("rename_file")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_review_direct_rename_rejects_stale_hash() -> anyhow::Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.hashline.enabled = true;
    }))
    .await?;

    let old_name = "hashline-direct-stale-rename.txt";
    let new_name = "hashline-direct-stale-renamed.txt";
    let original = "before\n";
    let changed = "changed\n";
    harness.write_file(old_name, original).await?;
    let stale_hash = hashline_file_hash(original);
    harness.write_file(old_name, changed).await?;

    let output = run_hashline_tool(
        &harness,
        "hashline-direct-stale-rename-call",
        "rename_file",
        &json!({
            "path": old_name,
            "new_path": new_name,
            "expected_hash": stale_hash,
        }),
        "rename with the stale hash",
    )
    .await?;

    assert!(output.contains("file hash mismatch"));
    assert_eq!(harness.read_file_text(old_name).await?, changed);
    assert!(!harness.path_exists(new_name).await?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_review_truncated_find_block_keeps_full_span_and_replayable_anchor()
-> anyhow::Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.hashline.enabled = true;
    }))
    .await?;

    let file_name = "hashline-oversized-block.rs";
    let anchor_line = format!("    let value_000 = \"{}\";", "x".repeat(120));
    let mut contents = String::from("fn oversized() {\n");
    contents.push_str(&anchor_line);
    contents.push('\n');
    for index in 1..350 {
        contents.push_str(&format!(
            "    let value_{index:03} = \"{}\";\n",
            "x".repeat(120)
        ));
    }
    contents.push_str("}\n");
    harness.write_file(file_name, &contents).await?;

    let first_output = run_hashline_tool(
        &harness,
        "hashline-oversized-find-call",
        "find_block",
        &json!({
            "path": file_name,
            "anchor": format!("2:{}", hashline_line_hash(&anchor_line)),
            "max_lines": 300,
        }),
        "find the oversized block",
    )
    .await?;
    let first: Value = serde_json::from_str(&first_output)?;
    let replay_anchor = first["block_anchor"]
        .as_str()
        .expect("find_block should return a replayable block anchor");

    assert_eq!(first["start_line"], json!(1));
    assert_eq!(first["end_line"], json!(352));
    assert_eq!(first["line_count"], json!(352));
    assert_eq!(first["truncated"], json!(true));
    assert!(
        first["block_lines"]
            .as_array()
            .is_some_and(|lines| lines.len() < 352)
    );
    assert!(
        first_output.len() <= 26 * 1024,
        "find_block output was {} bytes",
        first_output.len()
    );

    let file_header = first["header"]
        .as_str()
        .expect("find_block should return a replayable file header");
    let replay_output = run_hashline_tool(
        &harness,
        "hashline-oversized-find-replay-call",
        "patch",
        &json!({
            "path": file_name,
            "patch": format!(
                "{file_header}\nSWAP.BLK {replay_anchor}:\n+fn replacement() {{\n+}}"
            ),
        }),
        "replace the oversized block with its replayable anchor",
    )
    .await?;
    let replay: Value = serde_json::from_str(&replay_output)?;
    assert_eq!(replay["success"], json!(true));
    assert_eq!(
        harness.read_file_text(file_name).await?,
        "fn replacement() {\n}\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_review_actual_bracket_hash_path_edits_end_to_end() -> anyhow::Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.hashline.enabled = true;
    }))
    .await?;

    let file_name = "hashline]#actual.txt";
    let original = "alpha\nbeta\n";
    harness.write_file(file_name, original).await?;
    let output = run_hashline_tool(
        &harness,
        "hashline-bracket-hash-path-call",
        "patch",
        &json!({
            "path": file_name,
            "patch": format!(
                "[{file_name}]#{}\nSWAP 2:{}\n+bravo",
                hashline_file_hash(original),
                hashline_line_hash("beta"),
            ),
        }),
        "edit the actual bracket-hash path",
    )
    .await?;

    assert_eq!(harness.read_file_text(file_name).await?, "alpha\nbravo\n");
    let body: Value = serde_json::from_str(&output)?;
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["path"], json!(file_name));
    assert!(
        body["header"]
            .as_str()
            .is_some_and(|header| header.starts_with(&format!("[{file_name}]#")))
    );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hashline_write_dry_run_accepts_representation_only_change() -> anyhow::Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config.hashline.enabled = true;
    }))
    .await?;

    let file_name = "hashline-dry-representation.txt";
    let original = "\u{feff}alpha\r\nbeta\r\n";
    harness.write_file(file_name, original).await?;

    let output = run_hashline_tool(
        &harness,
        "hashline-dry-representation-call",
        "write",
        &json!({
            "path": file_name,
            "content": "alpha\nbeta\n",
            "force": true,
            "dry_run": true
        }),
        "dry run a representation-only hashline write",
    )
    .await?;
    let output: Value = serde_json::from_str(&output)?;

    assert_eq!(output["success"], json!(true));
    assert_eq!(output["dry_run"], json!(true));
    assert_eq!(output["preview"], Value::Null);
    assert_eq!(harness.read_file_text(file_name).await?, original);
    Ok(())
}
