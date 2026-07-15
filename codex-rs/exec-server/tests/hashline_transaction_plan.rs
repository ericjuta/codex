#![cfg(target_os = "linux")]

mod common;

use codex_exec_server::Environment;
use codex_exec_server::ExecServerError;
use codex_exec_server::ExecServerRuntimePaths;
use codex_exec_server::FileSystemSandboxContext;
use codex_exec_server::HashlineTransactionPlanRequest;
use codex_exec_server::LOCAL_ENVIRONMENT_ID;
use codex_exec_server::REMOTE_ENVIRONMENT_ID;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE;
use codex_hashline_transaction::FileMutation;
use codex_hashline_transaction::PlanSummary;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::protocol::SandboxPolicy;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use common::current_test_binary_helper_paths;
use common::exec_server::exec_server;
use pretty_assertions::assert_eq;

fn root_uri(temp: &tempfile::TempDir) -> anyhow::Result<PathUri> {
    let root = AbsolutePathBuf::from_absolute_path_checked(temp.path())?;
    Ok(PathUri::from_abs_path(&root))
}

fn create_request(
    root: PathUri,
    sandbox: FileSystemSandboxContext,
) -> HashlineTransactionPlanRequest {
    HashlineTransactionPlanRequest {
        root,
        mutations: vec![FileMutation::Create {
            path: "new.txt".to_string(),
            contents: b"planned contents".to_vec(),
        }],
        sandbox: Some(sandbox),
    }
}

fn legacy_read_only_sandbox(root: PathUri) -> anyhow::Result<FileSystemSandboxContext> {
    Ok(FileSystemSandboxContext::from_legacy_sandbox_policy(
        SandboxPolicy::ReadOnly {
            network_access: false,
        },
        root,
    )?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restricted_planning_runs_in_the_helper_without_writing() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root_uri = root_uri(&temp)?;
    let server = exec_server().await?;
    let environment = Environment::create_for_tests(Some(server.websocket_url().to_string()))?;
    let response = environment
        .plan_hashline_transaction(create_request(
            root_uri.clone(),
            legacy_read_only_sandbox(root_uri)?,
        ))
        .await?;
    assert_eq!(response.preview.environment_id, REMOTE_ENVIRONMENT_ID);
    assert_eq!(
        response.preview.summary,
        PlanSummary {
            creates: 1,
            after_bytes: 16,
            ..PlanSummary::default()
        }
    );
    assert!(!temp.path().join("new.txt").exists());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_local_planning_runs_in_the_helper_without_writing() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root_uri = root_uri(&temp)?;
    let (codex_self_exe, codex_linux_sandbox_exe) = current_test_binary_helper_paths()?;
    let runtime_paths = ExecServerRuntimePaths::new(codex_self_exe, codex_linux_sandbox_exe)?;
    let environment = Environment::create(/*exec_server_url*/ None, runtime_paths)?;

    let response = environment
        .plan_hashline_transaction(create_request(
            root_uri.clone(),
            legacy_read_only_sandbox(root_uri)?,
        ))
        .await?;

    assert_eq!(response.preview.environment_id, LOCAL_ENVIRONMENT_ID);
    assert_eq!(response.preview.summary.creates, 1);
    assert!(!temp.path().join("new.txt").exists());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_planning_enforces_helper_read_permissions() -> anyhow::Result<()> {
    let allowed = tempfile::tempdir()?;
    let denied = tempfile::tempdir()?;
    let allowed_root = AbsolutePathBuf::from_absolute_path_checked(allowed.path())?;
    let allowed_uri = PathUri::from_abs_path(&allowed_root);
    let sandbox = FileSystemSandboxContext::from_permission_profile_with_cwd(
        PermissionProfile::from_runtime_permissions(
            &FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
                path: FileSystemPath::Path { path: allowed_root },
                access: FileSystemAccessMode::Read,
            }]),
            NetworkSandboxPolicy::Restricted,
        ),
        allowed_uri,
    );
    let server = exec_server().await?;
    let environment = Environment::create_for_tests(Some(server.websocket_url().to_string()))?;

    let error = environment
        .plan_hashline_transaction(create_request(root_uri(&denied)?, sandbox))
        .await
        .expect_err("sandbox helper should deny an unlisted transaction root");

    let ExecServerError::Server { code, message } = error else {
        anyhow::bail!("expected server error, got {error:?}");
    };
    assert_eq!(code, HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE, "{message}");
    assert!(!denied.path().join("new.txt").exists());
    Ok(())
}
