#![cfg(target_os = "linux")]

mod common;

use codex_exec_server::Environment;
use codex_exec_server::HashlineTransactionExecuteAction;
use codex_exec_server::HashlineTransactionExecuteRequest;
use codex_exec_server::HashlineTransactionRecoverRequest;
use codex_exec_server::REMOTE_ENVIRONMENT_ID;
use codex_exec_server_protocol::HashlineTransactionExecutionOutcome;
use codex_hashline_transaction::FileMutation;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use common::exec_server::exec_server;
use pretty_assertions::assert_eq;

fn root_uri(temp: &tempfile::TempDir) -> anyhow::Result<PathUri> {
    let root = AbsolutePathBuf::from_absolute_path_checked(temp.path())?;
    Ok(PathUri::from_abs_path(&root))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_environment_commits_and_exposes_bounded_root_recovery() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = root_uri(&temp)?;
    let server = exec_server().await?;
    let environment = Environment::create_for_tests(Some(server.websocket_url().to_string()))?;

    let response = environment
        .execute_hashline_transaction(HashlineTransactionExecuteRequest {
            root: root.clone(),
            mutations: vec![FileMutation::Create {
                path: "remote-commit.txt".to_string(),
                contents: b"remote committed contents".to_vec(),
            }],
            action: HashlineTransactionExecuteAction::Commit,
            sandbox: None,
        })
        .await?;

    assert_eq!(response.preview.environment_id, REMOTE_ENVIRONMENT_ID);
    assert_eq!(
        response.outcome,
        HashlineTransactionExecutionOutcome::Committed
    );
    assert_eq!(
        std::fs::read(temp.path().join("remote-commit.txt"))?,
        b"remote committed contents"
    );
    let recovery = environment
        .recover_hashline_transactions(HashlineTransactionRecoverRequest {
            root,
            sandbox: None,
        })
        .await?;
    assert_eq!(recovery.attempts, Vec::new());
    Ok(())
}
