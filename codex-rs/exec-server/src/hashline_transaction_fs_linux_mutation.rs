use std::ffi::OsStr;
use std::fs::File;
use std::num::NonZeroU64;

use codex_hashline_transaction::FileEvidence;
use codex_hashline_transaction::GuardedMutation;
use codex_hashline_transaction::GuardedRollback;
use codex_hashline_transaction::MetadataSnapshot;
use codex_hashline_transaction::MutationOutcome;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_hashline_transaction::TransactionMutation;

use super::NativeResolvedPath;
use super::identity_bytes;
use super::open_component;
use super::run_blocking;
use super::storage_io::apply_staged_metadata;
use super::storage_io::platform_error;
use super::verify_directory_chain;
use crate::hashline_transaction_fs::NativeTransactionFileSystem;

#[path = "hashline_transaction_fs_linux_mutation_io.rs"]
mod io_support;

use io_support::Artifact;
use io_support::EntryState;
use io_support::Guard;
use io_support::backup_artifact;
use io_support::changed;
use io_support::classify_entry_error;
use io_support::entry_state;
use io_support::link_artifact;
use io_support::matches_evidence;
use io_support::path_state;
use io_support::rename_with_flags;
use io_support::require_journal;
use io_support::staged_artifact;
use io_support::temporary_name;
use io_support::unlink_entry;

impl TransactionMutation for NativeTransactionFileSystem {
    async fn apply_guarded(
        &self,
        lease: &Self::Lease,
        journal: &Self::Journal,
        mutation: GuardedMutation<'_, Self::ResolvedPath, Self::StagedFile, Self::Backup>,
    ) -> Result<MutationOutcome, TransactionFileSystemError> {
        require_journal(journal)?;
        match mutation {
            GuardedMutation::Create {
                destination,
                staged,
            } => {
                lease.require_path(destination, "create transaction file")?;
                let destination = destination.clone();
                let staged = staged_artifact(self, staged)?;
                run_blocking("create transaction file", move || {
                    apply_create(&destination, &staged)
                })
                .await
            }
            GuardedMutation::Replace {
                destination,
                expected,
                staged,
                backup,
            } => {
                lease.require_path(destination, "replace transaction file")?;
                let destination = destination.clone();
                let expected = expected.clone();
                let staged = staged_artifact(self, staged)?;
                let backup = backup_artifact(self, backup)?;
                run_blocking("replace transaction file", move || {
                    backup.validate_before(&expected)?;
                    exchange_replace(&destination, &staged, Guard::Evidence(expected), "apply")
                })
                .await
            }
            GuardedMutation::Remove {
                source,
                expected,
                backup,
            } => {
                lease.require_path(source, "remove transaction file")?;
                let source = source.clone();
                let expected = expected.clone();
                let backup = backup_artifact(self, backup)?;
                run_blocking("remove transaction file", move || {
                    backup.validate_before(&expected)?;
                    remove_path_guarded(&source, Guard::Evidence(expected))
                })
                .await
            }
            GuardedMutation::Move {
                source,
                expected,
                destination,
                staged,
                backup,
            } => {
                lease.require_path(source, "move transaction source")?;
                lease.require_path(destination, "move transaction destination")?;
                let staged = staged.ok_or_else(|| TransactionFileSystemError::Platform {
                    operation: "move transaction file",
                    reason: "move is missing its staged after-image".to_string(),
                })?;
                let source = source.clone();
                let destination = destination.clone();
                let expected = expected.clone();
                let staged = staged_artifact(self, staged)?;
                let backup = backup_artifact(self, backup)?;
                run_blocking("move transaction file", move || {
                    backup.validate_before(&expected)?;
                    apply_move(&source, &destination, &expected, &staged)
                })
                .await
            }
        }
    }

    async fn restore_guarded(
        &self,
        lease: &Self::Lease,
        journal: &Self::Journal,
        rollback: GuardedRollback<'_, Self::ResolvedPath, Self::StagedFile, Self::Backup>,
    ) -> Result<MutationOutcome, TransactionFileSystemError> {
        require_journal(journal)?;
        match rollback {
            GuardedRollback::RemoveCreated {
                destination,
                staged,
            } => {
                lease.require_path(destination, "rollback created transaction file")?;
                let destination = destination.clone();
                let staged = staged_artifact(self, staged)?;
                run_blocking("rollback created transaction file", move || {
                    remove_path_guarded(&destination, Guard::Artifact(staged))
                })
                .await
            }
            GuardedRollback::RestoreReplaced {
                destination,
                expected,
                staged,
                backup,
            } => {
                lease.require_path(destination, "rollback replaced transaction file")?;
                let destination = destination.clone();
                let expected = expected.clone();
                let staged = staged_artifact(self, staged)?;
                let backup = backup_artifact(self, backup)?;
                run_blocking("rollback replaced transaction file", move || {
                    backup.validate_before(&expected)?;
                    let state = path_state(&destination)?;
                    if matches_evidence(&state, &expected) {
                        return Ok(MutationOutcome::AlreadyApplied);
                    }
                    exchange_replace(&destination, &backup, Guard::Artifact(staged), "rollback")
                })
                .await
            }
            GuardedRollback::RestoreRemoved {
                source,
                expected,
                backup,
            } => {
                lease.require_path(source, "rollback removed transaction file")?;
                let source = source.clone();
                let expected = expected.clone();
                let backup = backup_artifact(self, backup)?;
                run_blocking("rollback removed transaction file", move || {
                    backup.validate_before(&expected)?;
                    restore_removed(&source, &expected, &backup)
                })
                .await
            }
            GuardedRollback::RestoreMove {
                source,
                expected,
                destination,
                staged,
                backup,
            } => {
                lease.require_path(source, "rollback moved transaction source")?;
                lease.require_path(destination, "rollback moved transaction destination")?;
                let source = source.clone();
                let destination = destination.clone();
                let expected = expected.clone();
                let staged = staged_artifact(self, staged)?;
                let backup = backup_artifact(self, backup)?;
                run_blocking("rollback moved transaction file", move || {
                    backup.validate_before(&expected)?;
                    restore_move(&source, &destination, &expected, &staged, &backup)
                })
                .await
            }
        }
    }

    async fn sync_parent(
        &self,
        path: &Self::ResolvedPath,
    ) -> Result<(), TransactionFileSystemError> {
        let path = path.clone();
        run_blocking("sync transaction parent", move || {
            verify_directory_chain(&path)?;
            path.parent
                .sync_all()
                .map_err(|error| platform_error("sync transaction parent", error))
        })
        .await
    }

    async fn restore_metadata(
        &self,
        path: &Self::ResolvedPath,
        metadata: &MetadataSnapshot,
    ) -> Result<(), TransactionFileSystemError> {
        let path = path.clone();
        let metadata = metadata.clone();
        run_blocking("restore transaction metadata", move || {
            verify_directory_chain(&path)?;
            let file = open_component(&path.parent, &path.final_component, super::OpenKind::Any)
                .map_err(|error| classify_entry_error(&path, error))?;
            let identity =
                identity_bytes(&file.metadata().map_err(|error| {
                    platform_error("inspect transaction metadata target", error)
                })?);
            apply_staged_metadata(&file, Some(&metadata))?;
            file.sync_all()
                .map_err(|error| platform_error("sync transaction metadata target", error))?;
            let rebound = open_component(&path.parent, &path.final_component, super::OpenKind::Any)
                .map_err(|error| classify_entry_error(&path, error))?;
            let rebound_identity =
                identity_bytes(&rebound.metadata().map_err(|error| {
                    platform_error("reinspect transaction metadata target", error)
                })?);
            if identity != rebound_identity {
                return Err(changed(&path.model_path));
            }
            Ok(())
        })
        .await
    }
}

fn apply_create(
    destination: &NativeResolvedPath,
    staged: &Artifact,
) -> Result<MutationOutcome, TransactionFileSystemError> {
    let artifact = staged.validate()?;
    let state = path_state(destination)?;
    if staged.matches_linked(&state) {
        return Ok(MutationOutcome::AlreadyApplied);
    }
    if !matches!(state, EntryState::Absent) || artifact.link_count != NonZeroU64::MIN {
        return Err(changed(&destination.model_path));
    }
    link_artifact(staged, &destination.parent, &destination.final_component)?;
    let state = path_state(destination)?;
    if !staged.matches_linked(&state) {
        return Err(changed(&destination.model_path));
    }
    Ok(MutationOutcome::Applied)
}

fn apply_move(
    source: &NativeResolvedPath,
    destination: &NativeResolvedPath,
    expected: &FileEvidence,
    staged: &Artifact,
) -> Result<MutationOutcome, TransactionFileSystemError> {
    staged.validate()?;
    let source_state = path_state(source)?;
    let destination_state = path_state(destination)?;
    if matches!(source_state, EntryState::Absent) && staged.matches_linked(&destination_state) {
        return Ok(MutationOutcome::AlreadyApplied);
    }
    if !matches_evidence(&source_state, expected) {
        return Err(changed(&source.model_path));
    }
    if matches!(destination_state, EntryState::Absent) {
        link_artifact(staged, &destination.parent, &destination.final_component)?;
    } else if !staged.matches_linked(&destination_state) {
        return Err(changed(&destination.model_path));
    }
    remove_path_guarded(source, Guard::Evidence(expected.clone()))?;
    Ok(MutationOutcome::Applied)
}

fn restore_removed(
    source: &NativeResolvedPath,
    expected: &FileEvidence,
    backup: &Artifact,
) -> Result<MutationOutcome, TransactionFileSystemError> {
    let state = path_state(source)?;
    if matches_evidence(&state, expected) || backup.matches_linked(&state) {
        return Ok(MutationOutcome::AlreadyApplied);
    }
    if !matches!(state, EntryState::Absent) {
        return Err(changed(&source.model_path));
    }
    link_artifact(backup, &source.parent, &source.final_component)?;
    Ok(MutationOutcome::Applied)
}

fn restore_move(
    source: &NativeResolvedPath,
    destination: &NativeResolvedPath,
    expected: &FileEvidence,
    staged: &Artifact,
    backup: &Artifact,
) -> Result<MutationOutcome, TransactionFileSystemError> {
    let source_state = path_state(source)?;
    let destination_state = path_state(destination)?;
    if (matches_evidence(&source_state, expected) || backup.matches_linked(&source_state))
        && matches!(destination_state, EntryState::Absent)
    {
        return Ok(MutationOutcome::AlreadyApplied);
    }
    if !matches!(source_state, EntryState::Absent) || !staged.matches_linked(&destination_state) {
        return Err(changed(&source.model_path));
    }
    link_artifact(backup, &source.parent, &source.final_component)?;
    remove_path_guarded(destination, Guard::Artifact(staged.clone()))?;
    Ok(MutationOutcome::Applied)
}

fn exchange_replace(
    destination: &NativeResolvedPath,
    desired: &Artifact,
    displaced: Guard,
    role: &str,
) -> Result<MutationOutcome, TransactionFileSystemError> {
    desired.validate()?;
    let temporary = temporary_name("exchange", role, desired.identity());
    let destination_state = path_state(destination)?;
    let temporary_state = entry_state(&destination.parent, &temporary, &destination.model_path)?;

    if desired.matches_linked(&destination_state) {
        if matches!(temporary_state, EntryState::Absent) {
            return Ok(MutationOutcome::AlreadyApplied);
        }
        if displaced.matches(&temporary_state) {
            remove_entry_guarded(
                &destination.parent,
                &temporary,
                &destination.model_path,
                displaced,
            )?;
            return Ok(MutationOutcome::Applied);
        }
        return Err(changed(&destination.model_path));
    }
    if !displaced.matches(&destination_state) {
        return Err(changed(&destination.model_path));
    }
    if matches!(temporary_state, EntryState::Absent) {
        link_artifact(desired, &destination.parent, &temporary)?;
    } else if !desired.matches_linked(&temporary_state) {
        return Err(changed(&destination.model_path));
    }
    rename_with_flags(
        &destination.parent,
        &temporary,
        &destination.parent,
        &destination.final_component,
        libc::RENAME_EXCHANGE,
        "exchange transaction file",
    )?;
    let destination_state = path_state(destination)?;
    let temporary_state = entry_state(&destination.parent, &temporary, &destination.model_path)?;
    if !desired.matches_linked(&destination_state) || !displaced.matches(&temporary_state) {
        if desired.matches_linked(&destination_state) {
            let _ = rename_with_flags(
                &destination.parent,
                &temporary,
                &destination.parent,
                &destination.final_component,
                libc::RENAME_EXCHANGE,
                "restore disturbed transaction exchange",
            );
        }
        return Err(changed(&destination.model_path));
    }
    remove_entry_guarded(
        &destination.parent,
        &temporary,
        &destination.model_path,
        displaced,
    )?;
    Ok(MutationOutcome::Applied)
}

fn remove_path_guarded(
    path: &NativeResolvedPath,
    guard: Guard,
) -> Result<MutationOutcome, TransactionFileSystemError> {
    verify_directory_chain(path)?;
    remove_entry_guarded(&path.parent, &path.final_component, &path.model_path, guard)
}

fn remove_entry_guarded(
    parent: &File,
    name: &OsStr,
    label: &str,
    guard: Guard,
) -> Result<MutationOutcome, TransactionFileSystemError> {
    let tomb = temporary_name("remove", "guarded", guard.identity());
    let state = entry_state(parent, name, label)?;
    let tomb_state = entry_state(parent, &tomb, label)?;
    if matches!(state, EntryState::Absent) && matches!(tomb_state, EntryState::Absent) {
        return Ok(MutationOutcome::AlreadyApplied);
    }
    if matches!(state, EntryState::Absent) && guard.matches(&tomb_state) {
        unlink_entry(parent, &tomb, "finalize guarded transaction removal")?;
        return Ok(MutationOutcome::Applied);
    }
    if !guard.matches(&state) || !matches!(tomb_state, EntryState::Absent) {
        return Err(changed(label));
    }
    rename_with_flags(
        parent,
        name,
        parent,
        &tomb,
        libc::RENAME_NOREPLACE,
        "guard transaction removal",
    )?;
    let tomb_state = entry_state(parent, &tomb, label)?;
    if !guard.matches(&tomb_state) {
        let _ = rename_with_flags(
            parent,
            &tomb,
            parent,
            name,
            libc::RENAME_NOREPLACE,
            "restore disturbed transaction removal",
        );
        return Err(changed(label));
    }
    unlink_entry(parent, &tomb, "finalize guarded transaction removal")?;
    Ok(MutationOutcome::Applied)
}

#[cfg(test)]
#[path = "hashline_transaction_fs_linux_mutation_tests.rs"]
mod tests;
