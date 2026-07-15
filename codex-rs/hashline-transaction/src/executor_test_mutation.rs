use crate::FileEvidence;
use crate::GuardedMutation;
use crate::GuardedRollback;
use crate::MutationOutcome;
use crate::TransactionFileSystemError;

use super::executor_test_support::TestBackup;
use super::executor_test_support::TestStagedFile;
use super::executor_test_support::TestState;
use super::executor_test_support::changed;
use super::executor_test_support::platform;

pub(super) fn apply(
    state: &mut TestState,
    mutation: GuardedMutation<'_, String, TestStagedFile, TestBackup>,
) -> Result<(String, MutationOutcome), TransactionFileSystemError> {
    match mutation {
        GuardedMutation::Create {
            destination,
            staged,
        } => {
            require_destination(staged, destination)?;
            if state.files.get(destination) == Some(&staged.file) {
                return Ok((
                    format!("create:{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            if state.files.contains_key(destination) {
                return Err(changed(destination));
            }
            state.files.insert(destination.clone(), staged.file.clone());
            Ok((format!("create:{destination}"), MutationOutcome::Applied))
        }
        GuardedMutation::Replace {
            destination,
            expected,
            staged,
            backup,
        } => {
            require_destination(staged, destination)?;
            require_backup(backup, destination, expected)?;
            if state.files.get(destination) == Some(&staged.file) {
                return Ok((
                    format!("replace:{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            require_file(state, destination, expected, "replace")?;
            state.files.insert(destination.clone(), staged.file.clone());
            Ok((format!("replace:{destination}"), MutationOutcome::Applied))
        }
        GuardedMutation::Remove {
            source,
            expected,
            backup,
        } => {
            require_backup(backup, source, expected)?;
            if !state.files.contains_key(source) {
                return Ok((format!("remove:{source}"), MutationOutcome::AlreadyApplied));
            }
            require_file(state, source, expected, "remove")?;
            state.files.remove(source);
            Ok((format!("remove:{source}"), MutationOutcome::Applied))
        }
        GuardedMutation::Move {
            source,
            expected,
            destination,
            staged,
            backup,
        } => {
            let staged = staged.ok_or_else(|| platform("move", "missing staged after-image"))?;
            require_destination(staged, destination)?;
            require_backup(backup, source, expected)?;
            if !state.files.contains_key(source)
                && state.files.get(destination) == Some(&staged.file)
            {
                return Ok((
                    format!("move:{source}->{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            require_file(state, source, expected, "move")?;
            if state.files.contains_key(destination) {
                return Err(changed(destination));
            }
            state.files.remove(source);
            state.files.insert(destination.clone(), staged.file.clone());
            Ok((
                format!("move:{source}->{destination}"),
                MutationOutcome::Applied,
            ))
        }
    }
}

pub(super) fn restore(
    state: &mut TestState,
    rollback: GuardedRollback<'_, String, TestStagedFile, TestBackup>,
) -> Result<(String, MutationOutcome), TransactionFileSystemError> {
    match rollback {
        GuardedRollback::RemoveCreated {
            destination,
            staged,
        } => {
            require_destination(staged, destination)?;
            if !state.files.contains_key(destination) {
                return Ok((
                    format!("create:{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            require_file(
                state,
                destination,
                &FileEvidence::from(&staged.file),
                "rollback create",
            )?;
            state.files.remove(destination);
            Ok((format!("create:{destination}"), MutationOutcome::Applied))
        }
        GuardedRollback::RestoreReplaced {
            destination,
            expected,
            staged,
            backup,
        } => {
            require_destination(staged, destination)?;
            require_backup(backup, destination, expected)?;
            if state
                .files
                .get(destination)
                .is_some_and(|file| FileEvidence::from(file) == *expected)
            {
                return Ok((
                    format!("replace:{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            if state.files.get(destination) == Some(&backup.file) {
                return Ok((
                    format!("replace:{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            require_file(
                state,
                destination,
                &FileEvidence::from(&staged.file),
                "rollback replace",
            )?;
            state.files.insert(destination.clone(), backup.file.clone());
            Ok((format!("replace:{destination}"), MutationOutcome::Applied))
        }
        GuardedRollback::RestoreRemoved {
            source,
            expected,
            backup,
        } => {
            require_backup(backup, source, expected)?;
            if state
                .files
                .get(source)
                .is_some_and(|file| FileEvidence::from(file) == *expected)
            {
                return Ok((format!("remove:{source}"), MutationOutcome::AlreadyApplied));
            }
            if state.files.get(source) == Some(&backup.file) {
                return Ok((format!("remove:{source}"), MutationOutcome::AlreadyApplied));
            }
            if state.files.contains_key(source) {
                return Err(changed(source));
            }
            state.files.insert(source.clone(), backup.file.clone());
            Ok((format!("remove:{source}"), MutationOutcome::Applied))
        }
        GuardedRollback::RestoreMove {
            source,
            expected,
            destination,
            staged,
            backup,
        } => {
            require_destination(staged, destination)?;
            require_backup(backup, source, expected)?;
            if state
                .files
                .get(source)
                .is_some_and(|file| FileEvidence::from(file) == *expected)
                && !state.files.contains_key(destination)
            {
                return Ok((
                    format!("move:{source}->{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            if state.files.get(source) == Some(&backup.file)
                && !state.files.contains_key(destination)
            {
                return Ok((
                    format!("move:{source}->{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            if state.files.contains_key(source) {
                return Err(changed(source));
            }
            require_file(
                state,
                destination,
                &FileEvidence::from(&staged.file),
                "rollback move",
            )?;
            state.files.remove(destination);
            state.files.insert(source.clone(), backup.file.clone());
            Ok((
                format!("move:{source}->{destination}"),
                MutationOutcome::Applied,
            ))
        }
    }
}

pub(super) fn require_file(
    state: &TestState,
    path: &str,
    expected: &FileEvidence,
    operation: &'static str,
) -> Result<(), TransactionFileSystemError> {
    if state
        .files
        .get(path)
        .is_some_and(|file| FileEvidence::from(file) == *expected)
    {
        Ok(())
    } else {
        Err(TransactionFileSystemError::ChangedSincePlanning {
            path: format!("{operation}:{path}"),
        })
    }
}

fn require_destination(
    staged: &TestStagedFile,
    destination: &str,
) -> Result<(), TransactionFileSystemError> {
    if staged.destination == destination {
        Ok(())
    } else {
        Err(platform("staged destination", destination.to_string()))
    }
}

fn require_backup(
    backup: &TestBackup,
    source: &str,
    expected: &FileEvidence,
) -> Result<(), TransactionFileSystemError> {
    if backup.source != source {
        return Err(platform("backup source", source.to_string()));
    }
    if backup.before == *expected {
        Ok(())
    } else {
        Err(changed(source))
    }
}
