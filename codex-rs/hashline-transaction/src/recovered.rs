use crate::CanonicalPathKey;
use crate::JournalMutation;
use crate::JournalOperation;
use crate::TransactionFileSystem;
use crate::TransactionFileSystemError;
use crate::prepared::PreparedMutation;

pub(crate) struct RecoveredEntry<P> {
    pub(crate) paths: RecoveredPaths<P>,
    pub(crate) operation: JournalOperation,
}

pub(crate) enum RecoveredPaths<P> {
    Create { destination: P },
    Update { path: P },
    Delete { path: P },
    Move { source: P, destination: P },
}

impl<P> RecoveredPaths<P> {
    pub(crate) fn paths(&self) -> Vec<&P> {
        match self {
            Self::Create { destination } => vec![destination],
            Self::Update { path } | Self::Delete { path } => vec![path],
            Self::Move {
                source,
                destination,
            } => vec![source, destination],
        }
    }
}

pub(crate) async fn reopen_entries<F: TransactionFileSystem>(
    file_system: &F,
    root: &F::Root,
    mutations: &[JournalMutation],
) -> Result<Vec<RecoveredEntry<F::ResolvedPath>>, TransactionFileSystemError> {
    let mut entries = Vec::with_capacity(mutations.len());
    for mutation in mutations {
        let operation = mutation.operation.clone();
        let paths = match &operation {
            JournalOperation::Create { destination, .. } => RecoveredPaths::Create {
                destination: file_system.reopen_path(root, destination).await?,
            },
            JournalOperation::Update { path, .. } => RecoveredPaths::Update {
                path: file_system.reopen_path(root, path).await?,
            },
            JournalOperation::Delete { path, .. } => RecoveredPaths::Delete {
                path: file_system.reopen_path(root, path).await?,
            },
            JournalOperation::Move {
                source,
                destination,
                ..
            } => RecoveredPaths::Move {
                source: file_system.reopen_path(root, source).await?,
                destination: file_system.reopen_path(root, destination).await?,
            },
        };
        entries.push(RecoveredEntry { paths, operation });
    }
    Ok(entries)
}

pub(crate) fn ordered_lock_paths<F: TransactionFileSystem>(
    file_system: &F,
    entries: &[RecoveredEntry<F::ResolvedPath>],
) -> Result<Vec<F::ResolvedPath>, TransactionFileSystemError> {
    let mut paths = Vec::<(CanonicalPathKey, F::ResolvedPath)>::new();
    for entry in entries {
        for path in entry.paths.paths() {
            paths.push((file_system.canonical_path_key(path)?, path.clone()));
        }
    }
    paths.sort_by(|(left, _), (right, _)| left.cmp(right));
    if paths.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(TransactionFileSystemError::ChangedSincePlanning {
            path: "durable recovery path alias".to_string(),
        });
    }
    Ok(paths.into_iter().map(|(_, path)| path).collect())
}

pub(crate) async fn reopen_rollback<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    entry: &RecoveredEntry<F::ResolvedPath>,
) -> Result<PreparedMutation<F::ResolvedPath, F::StagedFile, F::Backup>, TransactionFileSystemError>
{
    match (&entry.paths, &entry.operation) {
        (RecoveredPaths::Create { destination }, JournalOperation::Create { staged, .. }) => {
            Ok(PreparedMutation::Create {
                destination: destination.clone(),
                staged: file_system.reopen_staged_file(storage, staged).await?,
            })
        }
        (
            RecoveredPaths::Update { path },
            JournalOperation::Update {
                before,
                staged,
                backup,
                ..
            },
        ) => Ok(PreparedMutation::Replace {
            destination: path.clone(),
            expected: before.clone(),
            staged: file_system.reopen_staged_file(storage, staged).await?,
            backup: file_system.reopen_backup(storage, backup).await?,
        }),
        (RecoveredPaths::Delete { path }, JournalOperation::Delete { before, backup, .. }) => {
            Ok(PreparedMutation::Remove {
                source: path.clone(),
                expected: before.clone(),
                backup: file_system.reopen_backup(storage, backup).await?,
            })
        }
        (
            RecoveredPaths::Move {
                source,
                destination,
            },
            JournalOperation::Move {
                before,
                staged,
                backup,
                ..
            },
        ) => Ok(PreparedMutation::Move {
            source: source.clone(),
            expected: before.clone(),
            destination: destination.clone(),
            staged: file_system.reopen_staged_file(storage, staged).await?,
            backup: file_system.reopen_backup(storage, backup).await?,
        }),
        _ => Err(TransactionFileSystemError::Platform {
            operation: "reopen recovery mutation",
            reason: "journal operation and reopened paths disagree".to_string(),
        }),
    }
}
