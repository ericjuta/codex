use std::fs;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::Path;

use codex_exec_server_protocol::HASHLINE_TRANSACTION_RECOVERY_REQUIRED_ERROR_CODE;
use codex_hashline_transaction::FileMutation;
use codex_hashline_transaction::JournalRecord;
use codex_hashline_transaction::JournalState;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;

use super::*;

fn root_uri(root: &Path) -> PathUri {
    let root = AbsolutePathBuf::from_absolute_path_checked(root)
        .unwrap_or_else(|error| panic!("temporary directory should be absolute: {error}"));
    PathUri::from_abs_path(&root)
}

fn create_params(root: &Path, path: &str) -> HashlineTransactionExecuteParams {
    HashlineTransactionExecuteParams {
        environment_id: "execute-recovery-test".to_string(),
        root: root_uri(root),
        action: HashlineTransactionExecuteAction::Commit,
        mutations: vec![FileMutation::Create {
            path: path.to_string(),
            contents: format!("contents-{path}").into_bytes(),
        }],
        sandbox: None,
    }
}

fn journal_path(root: &Path, transaction_id: &TransactionId) -> std::path::PathBuf {
    root.join(".codex-hashline-transactions")
        .join(&transaction_id.0)
        .join("journal")
}

fn rewrite_as_interrupted_cleaning(journal_path: &Path) {
    let mut record: JournalRecord = serde_json::from_slice(
        &fs::read(journal_path).expect("read completed transaction journal"),
    )
    .expect("decode completed transaction journal");
    record.state = JournalState::Cleaning;
    record.validate().expect("cleaning journal should be valid");
    let bytes = serde_json::to_vec(&record).expect("encode cleaning journal");
    let mut journal = File::create(journal_path).expect("reopen transaction journal");
    journal.write_all(&bytes).expect("write cleaning journal");
    journal.sync_all().expect("sync cleaning journal");
    File::open(journal_path.parent().expect("journal parent"))
        .expect("open transaction directory")
        .sync_all()
        .expect("sync transaction directory");
}

#[tokio::test]
async fn next_commit_recovers_interrupted_cleanup_before_planning() {
    let temp = tempfile::tempdir().expect("create transaction root");
    let first = execute_direct(create_params(temp.path(), "first.txt"))
        .await
        .expect("commit first transaction");
    let first_journal = journal_path(temp.path(), &first.transaction_id);
    rewrite_as_interrupted_cleaning(&first_journal);

    execute_direct(create_params(temp.path(), "second.txt"))
        .await
        .expect("recover first transaction and commit second");

    assert_eq!(
        fs::read(temp.path().join("first.txt")).expect("read first result"),
        b"contents-first.txt"
    );
    assert_eq!(
        fs::read(temp.path().join("second.txt")).expect("read second result"),
        b"contents-second.txt"
    );
    let recovered: JournalRecord = serde_json::from_slice(
        &fs::read(first_journal).expect("read recovered transaction journal"),
    )
    .expect("decode recovered transaction journal");
    assert_eq!(recovered.state, JournalState::Complete);
}

#[tokio::test]
async fn tampered_journal_blocks_the_next_commit_without_following_symlink() {
    let temp = tempfile::tempdir().expect("create transaction root");
    let first = execute_direct(create_params(temp.path(), "first.txt"))
        .await
        .expect("commit first transaction");
    let first_journal = journal_path(temp.path(), &first.transaction_id);
    let outside = temp.path().join("outside");
    fs::write(&outside, b"outside-bytes").expect("write external fixture");
    fs::remove_file(&first_journal).expect("remove trusted journal");
    symlink(&outside, &first_journal).expect("replace journal with symlink");

    let error = execute_direct(create_params(temp.path(), "second.txt"))
        .await
        .expect_err("tampered recovery state must block a new commit");

    assert_eq!(
        error.code,
        HASHLINE_TRANSACTION_RECOVERY_REQUIRED_ERROR_CODE
    );
    assert_eq!(
        fs::read(outside).expect("read external fixture"),
        b"outside-bytes"
    );
    assert!(!temp.path().join("second.txt").exists());
}
