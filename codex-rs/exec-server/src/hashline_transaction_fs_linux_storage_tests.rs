use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use codex_hashline_transaction::FileEvidence;
use codex_hashline_transaction::JournalBytes;
use codex_hashline_transaction::ObservationLimit;
use codex_hashline_transaction::ObservedPath;
use codex_hashline_transaction::PlanningFileSystem;
use codex_hashline_transaction::StageFileRequest;
use codex_hashline_transaction::StorageRequirements;
use codex_hashline_transaction::TransactionCoordination;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_hashline_transaction::TransactionId;
use codex_hashline_transaction::TransactionRecovery;
use codex_hashline_transaction::TransactionStorage;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::NativeStorage;
use crate::hashline_transaction_fs::NativeTransactionFileSystem;

fn root_uri(temp: &TempDir) -> PathUri {
    let root = AbsolutePathBuf::from_absolute_path_checked(temp.path())
        .unwrap_or_else(|error| panic!("temporary directory should be absolute: {error}"));
    PathUri::from_abs_path(&root)
}

#[tokio::test]
async fn recovery_waits_for_the_active_transaction_reservation() {
    let temp = tempfile::tempdir().expect("create transaction root");
    let storage = allocate(
        &temp,
        "txn-recovery-lock",
        StorageRequirements {
            staged_bytes: 8,
            backup_bytes: 8,
            journal_bytes: 64,
        },
    )
    .await;
    let file_system = file_system(&temp);
    let key = file_system
        .durable_transaction_key(&storage)
        .expect("read durable transaction key");
    let root = root_uri(&temp);
    let (started_tx, started_rx) = mpsc::sync_channel(/*bound*/ 1);
    let (acquired_tx, acquired_rx) = mpsc::sync_channel(/*bound*/ 1);
    let worker = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create recovery worker runtime");
        let result = runtime.block_on(async move {
            let recovery_file_system =
                NativeTransactionFileSystem::new("local-test-environment".to_string(), root);
            started_tx.send(()).expect("report recovery start");
            let recovered_storage = recovery_file_system
                .lock_recovery_storage(&key)
                .await
                .map_err(|error| error.to_string())?;
            drop(recovered_storage);
            Ok::<(), String>(())
        });
        acquired_tx.send(result).expect("report recovery result");
    });

    started_rx
        .recv_timeout(Duration::from_secs(/*secs*/ 5))
        .expect("recovery worker should start");
    assert!(
        matches!(
            acquired_rx.recv_timeout(Duration::from_millis(/*millis*/ 250)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "recovery must wait while the active transaction owns its reservation"
    );
    drop(storage);
    assert_eq!(
        acquired_rx
            .recv_timeout(Duration::from_secs(/*secs*/ 5))
            .expect("recovery should acquire after active transaction release"),
        Ok(())
    );
    worker.join().expect("join recovery worker");
}

fn file_system(temp: &TempDir) -> NativeTransactionFileSystem {
    NativeTransactionFileSystem::new("local-test-environment".to_string(), root_uri(temp))
}

async fn allocate(
    temp: &TempDir,
    transaction_id: &str,
    requirements: StorageRequirements,
) -> NativeStorage {
    let file_system = file_system(temp);
    let root = file_system
        .open_root(&root_uri(temp))
        .await
        .expect("open transaction root");
    let lease = file_system
        .lock_paths(&root, &[])
        .await
        .expect("lock transaction root");
    file_system
        .allocate_storage(
            &lease,
            &TransactionId(transaction_id.to_string()),
            requirements,
        )
        .await
        .expect("allocate transaction storage")
}

#[tokio::test]
async fn allocates_owner_only_storage_and_rejects_duplicate_or_invalid_ids() {
    let temp = tempfile::tempdir().expect("create transaction root");
    let requirements = StorageRequirements {
        staged_bytes: 4,
        backup_bytes: 5,
        journal_bytes: 6,
    };
    let storage = allocate(&temp, "txn-1", requirements).await;
    let transaction_directory = temp
        .path()
        .join(".codex-hashline-transactions")
        .join("txn-1");

    assert_eq!(
        fs::metadata(&transaction_directory)
            .expect("inspect transaction directory")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(transaction_directory.join("reservation"))
            .expect("inspect reservation")
            .len(),
        requirements.total_bytes()
    );

    let file_system = file_system(&temp);
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("reopen root");
    let lease = file_system
        .lock_paths(&root, &[])
        .await
        .expect("relock root");
    let duplicate = file_system
        .allocate_storage(&lease, &TransactionId("txn-1".to_string()), requirements)
        .await
        .expect_err("duplicate transaction ID must fail");
    assert!(matches!(
        duplicate,
        TransactionFileSystemError::Platform {
            operation: "allocate transaction storage",
            ..
        }
    ));
    let invalid = file_system
        .allocate_storage(
            &lease,
            &TransactionId("../escape".to_string()),
            requirements,
        )
        .await
        .expect_err("invalid transaction ID must fail");
    assert!(matches!(
        invalid,
        TransactionFileSystemError::Platform {
            operation: "allocate transaction storage",
            ..
        }
    ));

    drop(storage);
}

#[tokio::test]
async fn stages_and_backs_up_exact_bytes_with_bounded_capacity() {
    let temp = tempfile::tempdir().expect("create transaction root");
    fs::write(temp.path().join("source.txt"), b"before").expect("write source fixture");
    let file_system = file_system(&temp);
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open transaction root");
    let source = file_system
        .resolve(&root, "source.txt")
        .await
        .expect("resolve source");
    let destination = file_system
        .resolve(&root, "destination.txt")
        .await
        .expect("resolve destination");
    let ObservedPath::Present(observed) = file_system
        .observe(&source, ObservationLimit { max_bytes: 64 })
        .await
        .expect("observe source")
    else {
        panic!("source fixture should exist");
    };
    let expected = FileEvidence::from(&observed);
    let lease = file_system
        .lock_paths(&root, &[source.clone(), destination.clone()])
        .await
        .expect("lock transaction paths");
    let storage = file_system
        .allocate_storage(
            &lease,
            &TransactionId("txn-stage".to_string()),
            StorageRequirements {
                staged_bytes: 5,
                backup_bytes: 6,
                journal_bytes: 64,
            },
        )
        .await
        .expect("allocate transaction storage");

    let staged = file_system
        .stage_file(
            &storage,
            StageFileRequest {
                destination: &destination,
                contents: b"after",
                metadata: None,
            },
        )
        .await
        .expect("stage after image");
    let backup = file_system
        .backup_file(&storage, &source, &expected)
        .await
        .expect("back up before image");

    assert_eq!(
        file_system
            .staged_file_evidence(&staged)
            .expect("read staged evidence")
            .evidence
            .exact_digest,
        codex_hashline_transaction::ExactBytesDigest::new(b"after")
    );
    let backup_evidence = file_system
        .backup_evidence(&backup)
        .expect("read backup evidence")
        .evidence;
    assert_eq!(
        (
            &backup_evidence.exact_digest,
            &backup_evidence.metadata,
            backup_evidence.link_count,
            backup_evidence.kind,
        ),
        (
            &expected.exact_digest,
            &expected.metadata,
            expected.link_count,
            expected.kind,
        )
    );
    assert_eq!(
        backup_evidence.identity.namespace,
        expected.identity.namespace
    );
    assert_eq!(
        fs::read(
            temp.path()
                .join(".codex-hashline-transactions/txn-stage/staged")
                .join(&staged.name)
        )
        .expect("read staged file"),
        b"after"
    );
    assert_eq!(
        fs::read(
            temp.path()
                .join(".codex-hashline-transactions/txn-stage/backups")
                .join(&backup.name)
        )
        .expect("read backup file"),
        b"before"
    );
    file_system
        .sync_staged_file(&staged)
        .await
        .expect("sync staged file");
    file_system.sync_backup(&backup).await.expect("sync backup");
    file_system
        .sync_storage(&storage)
        .await
        .expect("sync transaction storage");

    let error = file_system
        .stage_file(
            &storage,
            StageFileRequest {
                destination: &destination,
                contents: b"x",
                metadata: None,
            },
        )
        .await
        .expect_err("staged capacity must be enforced");
    assert!(matches!(error, TransactionFileSystemError::Platform { .. }));
}

#[tokio::test]
async fn atomically_replaces_journal_and_cleanup_preserves_it() {
    let temp = tempfile::tempdir().expect("create transaction root");
    let storage = allocate(
        &temp,
        "txn-journal",
        StorageRequirements {
            staged_bytes: 8,
            backup_bytes: 8,
            journal_bytes: 64,
        },
    )
    .await;
    let file_system = file_system(&temp);
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open transaction root");
    let destination = file_system
        .resolve(&root, "new.txt")
        .await
        .expect("resolve destination");
    file_system
        .stage_file(
            &storage,
            StageFileRequest {
                destination: &destination,
                contents: b"artifact",
                metadata: None,
            },
        )
        .await
        .expect("stage cleanup fixture");
    let first = JournalBytes::try_from_vec(b"first".to_vec(), /*max_bytes*/ 64)
        .expect("create first journal bytes");
    let second = JournalBytes::try_from_vec(b"second".to_vec(), /*max_bytes*/ 64)
        .expect("create second journal bytes");

    file_system
        .persist_journal(&storage, &first)
        .await
        .expect("persist first journal");
    let journal = file_system
        .persist_journal(&storage, &second)
        .await
        .expect("replace journal");
    file_system
        .sync_journal(&journal)
        .await
        .expect("sync journal");

    let transaction_directory = temp.path().join(".codex-hashline-transactions/txn-journal");
    assert_eq!(
        fs::read(transaction_directory.join("journal")).expect("read current journal"),
        b"second"
    );
    let names = fs::read_dir(&transaction_directory)
        .expect("enumerate transaction directory")
        .map(|entry| entry.expect("read transaction entry").file_name())
        .collect::<Vec<OsString>>();
    assert!(
        !names
            .iter()
            .any(|name| name.to_string_lossy().starts_with("journal-tmp-"))
    );

    file_system
        .cleanup_artifacts(&storage)
        .await
        .expect("cleanup transaction artifacts");

    assert_eq!(
        fs::read(transaction_directory.join("journal")).expect("journal should remain"),
        b"second"
    );
    assert_eq!(
        fs::metadata(transaction_directory.join("reservation"))
            .expect("reservation receipt should remain")
            .len(),
        0
    );
    assert_eq!(
        fs::read_dir(transaction_directory.join("staged"))
            .expect("enumerate staged directory")
            .count(),
        0
    );
    assert_eq!(
        fs::read_dir(transaction_directory.join("backups"))
            .expect("enumerate backup directory")
            .count(),
        0
    );
}
