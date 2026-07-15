#![cfg(target_os = "linux")]

use codex_exec_server::REMOTE_ENVIRONMENT_ID;
use codex_features::Feature;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::FileSystemSpecialPath;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::protocol::TurnEnvironmentSelection;
use codex_utils_absolute_path::AbsolutePathBuf;
use core_test_support::PathExt;
use core_test_support::responses::ResponseMock;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call_with_namespace;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::skip_if_no_remote_env;
use core_test_support::skip_if_target_windows;
use core_test_support::test_codex::TestCodexHarness;
use core_test_support::test_codex::local;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use tempfile::TempDir;
use wiremock::MockServer;

const MAX_MODEL_PREVIEW_BYTES: usize = 8 * 1024;

async fn mount_preview(server: &MockServer, call_id: &str, arguments: &Value) -> ResponseMock {
    mount_sse_sequence(
        server,
        vec![
            sse(vec![
                ev_response_created("transaction-preview-response"),
                ev_function_call_with_namespace(
                    call_id,
                    "hashline",
                    "transaction",
                    &arguments.to_string(),
                ),
                ev_completed("transaction-preview-response"),
            ]),
            sse(vec![
                ev_assistant_message("transaction-preview-message", "preview complete"),
                ev_completed("transaction-preview-final-response"),
            ]),
        ],
    )
    .await
}

fn exact_digest(contents: &str) -> String {
    format!("{:x}", Sha256::digest(contents.as_bytes()))
}

fn deny_path_profile(path: AbsolutePathBuf) -> PermissionProfile {
    PermissionProfile::from_runtime_permissions(
        &FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Path { path },
                access: FileSystemAccessMode::Deny,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(/*subpath*/ None),
                },
                access: FileSystemAccessMode::Read,
            },
        ]),
        NetworkSandboxPolicy::Restricted,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_is_executor_neutral_bounded_and_never_writes_mixed_mutations() -> anyhow::Result<()>
{
    skip_if_target_windows!(
        Ok(()),
        "native Hashline transaction planning requires Linux"
    );

    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::HashlineTransactions)
            .expect("Hashline transaction feature should be available");
    }))
    .await?;
    let fixtures = [
        ("update.txt", "update before\n"),
        ("delete.txt", "delete before\n"),
        ("move.txt", "move before\n"),
    ];
    for (path, contents) in fixtures {
        harness.write_file(path, contents).await?;
    }

    let call_id = "hashline-transaction-preview";
    let arguments = json!({
        "action": { "type": "preview" },
        "mutations": [
            { "type": "create", "path": "created.txt", "contents": "x".repeat(16 * 1024) },
            {
                "type": "update", "path": "update.txt",
                "expected": { "exactDigest": exact_digest("update before\n") },
                "edits": [{ "type": "replaceAll", "contents": "update after\n" }],
            },
            {
                "type": "delete", "path": "delete.txt",
                "expected": { "exactDigest": exact_digest("delete before\n") },
            },
            {
                "type": "move", "source": "move.txt", "destination": "moved.txt",
                "expected": { "exactDigest": exact_digest("move before\n") },
                "edits": [{ "type": "replaceAll", "contents": "move after\n" }],
            },
        ],
    });
    let response_mock = mount_preview(harness.server(), call_id, &arguments).await;
    harness
        .submit("preview a mixed Hashline transaction")
        .await?;

    let output_text = response_mock
        .last_request()
        .expect("final response request")
        .function_call_output_text(call_id)
        .expect("transaction preview output");
    assert!(output_text.len() <= MAX_MODEL_PREVIEW_BYTES);
    let output: Value = serde_json::from_str(&output_text)?;
    assert_eq!(
        output["environmentId"],
        harness
            .test()
            .executor_environment()
            .selection()
            .environment_id
    );
    assert_eq!(output["previewTruncated"], true);
    assert_eq!(
        output["summary"],
        json!({
            "creates": 1, "updates": 1, "deletes": 1, "moves": 1,
            "beforeBytes": 40, "afterBytes": 16_408,
        })
    );
    for (path, contents) in fixtures {
        assert_eq!(harness.read_file_text(path).await?, contents);
    }
    assert!(!harness.path_exists("created.txt").await?);
    assert!(!harness.path_exists("moved.txt").await?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn immediate_commit_executes_through_the_model_tool() -> anyhow::Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::HashlineTransactions)
            .expect("Hashline transaction feature should be available");
    }))
    .await?;
    let call_id = "hashline-transaction-commit";
    let arguments = json!({
        "action": { "type": "commit" },
        "mutations": [{
            "type": "create",
            "path": "committed.txt",
            "contents": "committed contents\n",
        }],
    });
    let response_mock = mount_preview(harness.server(), call_id, &arguments).await;

    harness.submit("commit a Hashline transaction").await?;

    let output: Value = serde_json::from_str(
        &response_mock
            .last_request()
            .expect("final response request")
            .function_call_output_text(call_id)
            .expect("transaction commit output"),
    )?;
    assert_eq!(output["outcome"]["type"], "committed");
    assert_eq!(output["preview"]["summary"]["creates"], 1);
    assert_eq!(
        harness.read_file_text("committed.txt").await?,
        "committed contents\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_forwards_the_turn_sandbox() -> anyhow::Result<()> {
    skip_if_target_windows!(
        Ok(()),
        "native Hashline transaction planning requires Linux"
    );

    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::HashlineTransactions)
            .expect("Hashline transaction feature should be available");
    }))
    .await?;
    let denied_path = harness.path("denied.txt");
    harness.write_file("denied.txt", "protected\n").await?;
    let call_id = "hashline-transaction-denied-preview";
    let arguments = json!({
        "action": { "type": "preview" },
        "mutations": [{
            "type": "delete", "path": "denied.txt",
            "expected": { "exactDigest": exact_digest("protected\n") },
        }],
    });
    let response_mock = mount_preview(harness.server(), call_id, &arguments).await;
    harness
        .submit_with_permission_profile(
            "preview a sandbox-denied Hashline transaction",
            deny_path_profile(AbsolutePathBuf::try_from(denied_path)?),
        )
        .await?;

    let output = response_mock
        .last_request()
        .expect("final response request")
        .function_call_output_text(call_id)
        .expect("transaction preview error");
    assert!(
        output.contains("exec-server rejected request (-32603)")
            && output.contains("Permission denied"),
        "expected sandbox-specific executor denial: {output}"
    );
    assert!(serde_json::from_str::<Value>(&output).is_err());
    assert_eq!(harness.read_file_text("denied.txt").await?, "protected\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_routes_to_the_selected_remote_environment() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_no_remote_env!(Ok(()));
    skip_if_target_windows!(
        Ok(()),
        "native Hashline transaction planning requires Linux"
    );

    let server = start_mock_server().await;
    let mut builder = test_codex().with_config(|config| {
        config
            .features
            .enable(Feature::HashlineTransactions)
            .expect("Hashline transaction feature should be available");
    });
    let test = builder.build_with_remote_and_local_env(&server).await?;
    let local_cwd = TempDir::new()?;
    let remote_selection = TurnEnvironmentSelection {
        environment_id: REMOTE_ENVIRONMENT_ID.to_string(),
        cwd: test.executor_environment().selection().cwd.clone(),
        workspace_roots: test
            .executor_environment()
            .selection()
            .workspace_roots
            .clone(),
    };
    let target = remote_selection.cwd.join("remote-preview.txt")?;
    let call_id = "hashline-transaction-remote-preview";
    let response_mock = mount_preview(
        &server,
        call_id,
        &json!({
            "action": { "type": "preview" },
            "environment_id": REMOTE_ENVIRONMENT_ID,
            "mutations": [{ "type": "create", "path": "remote-preview.txt", "contents": "planned\n" }],
        }),
    )
    .await;
    test.submit_turn_with_environments(
        "preview in the selected remote environment",
        Some(vec![local(local_cwd.path().abs()), remote_selection]),
    )
    .await?;

    let output: Value = serde_json::from_str(
        &response_mock
            .last_request()
            .expect("final response request")
            .function_call_output_text(call_id)
            .expect("remote transaction preview output"),
    )?;
    assert_eq!(output["environmentId"], REMOTE_ENVIRONMENT_ID);
    assert!(
        test.fs()
            .get_metadata(&target, /*sandbox*/ None)
            .await
            .is_err()
    );
    Ok(())
}
