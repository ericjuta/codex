use std::fs;
use std::os::unix::fs::PermissionsExt;

use codex_hashline_transaction::FileEvidence;
use codex_hashline_transaction::GuardedMutation;
use codex_hashline_transaction::GuardedRollback;
use codex_hashline_transaction::JournalBytes;
use codex_hashline_transaction::MutationOutcome;
use codex_hashline_transaction::ObservationLimit;
use codex_hashline_transaction::ObservedPath;
use codex_hashline_transaction::PlanningFileSystem;
use codex_hashline_transaction::StageFileRequest;
use codex_hashline_transaction::StorageRequirements;
use codex_hashline_transaction::TransactionCoordination;
use codex_hashline_transaction::TransactionId;
use codex_hashline_transaction::TransactionMutation;
use codex_hashline_transaction::TransactionStorage;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use crate::hashline_transaction_fs::NativeTransactionFileSystem;

fn root_uri(temp: &TempDir) -> PathUri {
    let root = AbsolutePathBuf::from_absolute_path_checked(temp.path())
        .unwrap_or_else(|error| panic!("temporary directory should be absolute: {error}"));
    PathUri::from_abs_path(&root)
}

fn file_system(temp: &TempDir) -> NativeTransactionFileSystem {
    NativeTransactionFileSystem::new("mutation-test-environment".to_string(), root_uri(temp))
}

async fn expected(
    file_system: &NativeTransactionFileSystem,
    path: &<NativeTransactionFileSystem as PlanningFileSystem>::ResolvedPath,
) -> FileEvidence {
    let ObservedPath::Present(observed) = file_system
        .observe(path, ObservationLimit { max_bytes: 1024 })
        .await
        .expect("observe fixture")
    else {
        panic!("fixture should exist");
    };
    FileEvidence::from(&observed)
}

#[tokio::test]
async fn applies_retries_and_rolls_back_mixed_mutations() {
    let temp = tempfile::tempdir().expect("create transaction root");
    fs::write(temp.path().join("update.txt"), b"before").expect("write update fixture");
    fs::write(temp.path().join("delete.txt"), b"delete").expect("write delete fixture");
    fs::write(temp.path().join("move.txt"), b"move").expect("write move fixture");

    let file_system = file_system(&temp);
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open transaction root");
    let create = file_system
        .resolve(&root, "create.txt")
        .await
        .expect("resolve create");
    let update = file_system
        .resolve(&root, "update.txt")
        .await
        .expect("resolve update");
    let delete = file_system
        .resolve(&root, "delete.txt")
        .await
        .expect("resolve delete");
    let move_source = file_system
        .resolve(&root, "move.txt")
        .await
        .expect("resolve move source");
    let move_destination = file_system
        .resolve(&root, "moved.txt")
        .await
        .expect("resolve move destination");
    let update_before = expected(&file_system, &update).await;
    let delete_before = expected(&file_system, &delete).await;
    let move_before = expected(&file_system, &move_source).await;
    let lease = file_system
        .lock_paths(
            &root,
            &[
                create.clone(),
                update.clone(),
                delete.clone(),
                move_source.clone(),
                move_destination.clone(),
            ],
        )
        .await
        .expect("lock mutation paths");
    let storage = file_system
        .allocate_storage(
            &lease,
            &TransactionId("mutation-mixed".to_string()),
            StorageRequirements {
                staged_bytes: 12,
                backup_bytes: 16,
                journal_bytes: 128,
            },
        )
        .await
        .expect("allocate transaction storage");
    let create_staged = file_system
        .stage_file(
            &storage,
            StageFileRequest {
                destination: &create,
                contents: b"new",
                metadata: None,
            },
        )
        .await
        .expect("stage create");
    let update_staged = file_system
        .stage_file(
            &storage,
            StageFileRequest {
                destination: &update,
                contents: b"after",
                metadata: Some(&update_before.metadata),
            },
        )
        .await
        .expect("stage update");
    let move_staged = file_system
        .stage_file(
            &storage,
            StageFileRequest {
                destination: &move_destination,
                contents: b"move",
                metadata: Some(&move_before.metadata),
            },
        )
        .await
        .expect("stage move");
    let update_backup = file_system
        .backup_file(&storage, &update, &update_before)
        .await
        .expect("backup update");
    let delete_backup = file_system
        .backup_file(&storage, &delete, &delete_before)
        .await
        .expect("backup delete");
    let move_backup = file_system
        .backup_file(&storage, &move_source, &move_before)
        .await
        .expect("backup move");
    let journal = file_system
        .persist_journal(
            &storage,
            &JournalBytes::try_from_vec(b"{}".to_vec(), 128).expect("bounded journal"),
        )
        .await
        .expect("persist journal");

    assert_eq!(
        file_system
            .apply_guarded(
                &lease,
                &journal,
                GuardedMutation::Create {
                    destination: &create,
                    staged: &create_staged,
                },
            )
            .await
            .expect("apply create"),
        MutationOutcome::Applied
    );
    assert_eq!(
        file_system
            .apply_guarded(
                &lease,
                &journal,
                GuardedMutation::Replace {
                    destination: &update,
                    expected: &update_before,
                    staged: &update_staged,
                    backup: &update_backup,
                },
            )
            .await
            .expect("apply replace"),
        MutationOutcome::Applied
    );
    assert_eq!(
        file_system
            .apply_guarded(
                &lease,
                &journal,
                GuardedMutation::Remove {
                    source: &delete,
                    expected: &delete_before,
                    backup: &delete_backup,
                },
            )
            .await
            .expect("apply remove"),
        MutationOutcome::Applied
    );
    assert_eq!(
        file_system
            .apply_guarded(
                &lease,
                &journal,
                GuardedMutation::Move {
                    source: &move_source,
                    expected: &move_before,
                    destination: &move_destination,
                    staged: Some(&move_staged),
                    backup: &move_backup,
                },
            )
            .await
            .expect("apply move"),
        MutationOutcome::Applied
    );

    assert_eq!(
        fs::read(temp.path().join("create.txt")).expect("read create"),
        b"new"
    );
    assert_eq!(
        fs::read(temp.path().join("update.txt")).expect("read update"),
        b"after"
    );
    assert!(!temp.path().join("delete.txt").exists());
    assert!(!temp.path().join("move.txt").exists());
    assert_eq!(
        fs::read(temp.path().join("moved.txt")).expect("read move"),
        b"move"
    );

    assert_eq!(
        file_system
            .apply_guarded(
                &lease,
                &journal,
                GuardedMutation::Create {
                    destination: &create,
                    staged: &create_staged,
                },
            )
            .await
            .expect("retry create"),
        MutationOutcome::AlreadyApplied
    );
    assert_eq!(
        file_system
            .apply_guarded(
                &lease,
                &journal,
                GuardedMutation::Move {
                    source: &move_source,
                    expected: &move_before,
                    destination: &move_destination,
                    staged: Some(&move_staged),
                    backup: &move_backup,
                },
            )
            .await
            .expect("retry move"),
        MutationOutcome::AlreadyApplied
    );

    file_system
        .restore_guarded(
            &lease,
            &journal,
            GuardedRollback::RestoreMove {
                source: &move_source,
                expected: &move_before,
                destination: &move_destination,
                staged: &move_staged,
                backup: &move_backup,
            },
        )
        .await
        .expect("rollback move");
    file_system
        .restore_guarded(
            &lease,
            &journal,
            GuardedRollback::RestoreRemoved {
                source: &delete,
                expected: &delete_before,
                backup: &delete_backup,
            },
        )
        .await
        .expect("rollback remove");
    file_system
        .restore_guarded(
            &lease,
            &journal,
            GuardedRollback::RestoreReplaced {
                destination: &update,
                expected: &update_before,
                staged: &update_staged,
                backup: &update_backup,
            },
        )
        .await
        .expect("rollback replace");
    file_system
        .restore_guarded(
            &lease,
            &journal,
            GuardedRollback::RemoveCreated {
                destination: &create,
                staged: &create_staged,
            },
        )
        .await
        .expect("rollback create");

    assert!(!temp.path().join("create.txt").exists());
    assert_eq!(
        fs::read(temp.path().join("update.txt")).expect("read restored update"),
        b"before"
    );
    assert_eq!(
        fs::read(temp.path().join("delete.txt")).expect("read restored delete"),
        b"delete"
    );
    assert_eq!(
        fs::read(temp.path().join("move.txt")).expect("read restored move"),
        b"move"
    );
    assert!(!temp.path().join("moved.txt").exists());
}

#[tokio::test]
async fn rollback_preserves_an_externally_disturbed_destination() {
    let temp = tempfile::tempdir().expect("create transaction root");
    let file_system = file_system(&temp);
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open transaction root");
    let destination = file_system
        .resolve(&root, "created.txt")
        .await
        .expect("resolve destination");
    let lease = file_system
        .lock_paths(&root, std::slice::from_ref(&destination))
        .await
        .expect("lock destination");
    let storage = file_system
        .allocate_storage(
            &lease,
            &TransactionId("mutation-disturbed".to_string()),
            StorageRequirements {
                staged_bytes: 7,
                backup_bytes: 0,
                journal_bytes: 128,
            },
        )
        .await
        .expect("allocate storage");
    let staged = file_system
        .stage_file(
            &storage,
            StageFileRequest {
                destination: &destination,
                contents: b"planned",
                metadata: None,
            },
        )
        .await
        .expect("stage create");
    let journal = file_system
        .persist_journal(
            &storage,
            &JournalBytes::try_from_vec(b"{}".to_vec(), 128).expect("bounded journal"),
        )
        .await
        .expect("persist journal");
    file_system
        .apply_guarded(
            &lease,
            &journal,
            GuardedMutation::Create {
                destination: &destination,
                staged: &staged,
            },
        )
        .await
        .expect("apply create");

    fs::remove_file(temp.path().join("created.txt")).expect("remove planned result");
    fs::write(temp.path().join("created.txt"), b"external").expect("write disturbed result");
    let error = file_system
        .restore_guarded(
            &lease,
            &journal,
            GuardedRollback::RemoveCreated {
                destination: &destination,
                staged: &staged,
            },
        )
        .await
        .expect_err("disturbed destination must fail rollback");

    assert!(matches!(
        error,
        codex_hashline_transaction::TransactionFileSystemError::ChangedSincePlanning { .. }
    ));
    assert_eq!(
        fs::read(temp.path().join("created.txt")).expect("read disturbed destination"),
        b"external"
    );
}

#[tokio::test]
async fn restores_metadata_and_syncs_the_retained_parent() {
    let temp = tempfile::tempdir().expect("create transaction root");
    let target = temp.path().join("mode.txt");
    fs::write(&target, b"mode").expect("write metadata fixture");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).expect("set initial mode");
    let file_system = file_system(&temp);
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open transaction root");
    let path = file_system
        .resolve(&root, "mode.txt")
        .await
        .expect("resolve target");
    let before = expected(&file_system, &path).await;

    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).expect("change mode");
    file_system
        .restore_metadata(&path, &before.metadata)
        .await
        .expect("restore metadata");
    file_system.sync_parent(&path).await.expect("sync parent");

    assert_eq!(
        fs::metadata(&target)
            .expect("inspect restored metadata")
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
}
