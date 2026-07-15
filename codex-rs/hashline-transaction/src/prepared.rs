use crate::FileEvidence;
use crate::GuardedMutation;
use crate::GuardedRollback;
use crate::JournalOperation;
use crate::PlannedMutation;
use crate::StageFileRequest;
use crate::TransactionFileSystem;
use crate::TransactionFileSystemError;

pub(crate) struct PreparedEntry<P, S, B> {
    pub(crate) mutation: PreparedMutation<P, S, B>,
    pub(crate) journal: JournalOperation,
}

pub(crate) enum PreparedMutation<P, S, B> {
    Create {
        destination: P,
        staged: S,
    },
    Replace {
        destination: P,
        expected: FileEvidence,
        staged: S,
        backup: B,
    },
    Remove {
        source: P,
        expected: FileEvidence,
        backup: B,
    },
    Move {
        source: P,
        expected: FileEvidence,
        destination: P,
        staged: S,
        backup: B,
    },
}

impl<P, S, B> PreparedMutation<P, S, B> {
    pub(crate) fn guarded(&self) -> GuardedMutation<'_, P, S, B> {
        match self {
            Self::Create {
                destination,
                staged,
            } => GuardedMutation::Create {
                destination,
                staged,
            },
            Self::Replace {
                destination,
                expected,
                staged,
                backup,
            } => GuardedMutation::Replace {
                destination,
                expected,
                staged,
                backup,
            },
            Self::Remove {
                source,
                expected,
                backup,
            } => GuardedMutation::Remove {
                source,
                expected,
                backup,
            },
            Self::Move {
                source,
                expected,
                destination,
                staged,
                backup,
            } => GuardedMutation::Move {
                source,
                expected,
                destination,
                staged: Some(staged),
                backup,
            },
        }
    }

    pub(crate) fn rollback(&self) -> GuardedRollback<'_, P, S, B> {
        match self {
            Self::Create {
                destination,
                staged,
            } => GuardedRollback::RemoveCreated {
                destination,
                staged,
            },
            Self::Replace {
                destination,
                staged,
                backup,
                ..
            } => GuardedRollback::RestoreReplaced {
                destination,
                staged,
                backup,
            },
            Self::Remove { source, backup, .. } => {
                GuardedRollback::RestoreRemoved { source, backup }
            }
            Self::Move {
                source,
                destination,
                staged,
                backup,
                ..
            } => GuardedRollback::RestoreMove {
                source,
                destination,
                staged,
                backup,
            },
        }
    }

    pub(crate) fn parent_paths(&self) -> Vec<&P> {
        match self {
            Self::Create { destination, .. } | Self::Replace { destination, .. } => {
                vec![destination]
            }
            Self::Remove { source, .. } => vec![source],
            Self::Move {
                source,
                destination,
                ..
            } => vec![source, destination],
        }
    }
}

pub(crate) async fn prepare<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    mutations: &[PlannedMutation<F::ResolvedPath>],
) -> Result<Vec<PreparedEntry<F::ResolvedPath, F::StagedFile, F::Backup>>, TransactionFileSystemError>
{
    let mut prepared = Vec::with_capacity(mutations.len());
    for mutation in mutations {
        let entry = match mutation {
            PlannedMutation::Create { path, contents, .. } => {
                let staged = stage(file_system, storage, path, contents, /*metadata*/ None).await?;
                let journal = JournalOperation::Create {
                    destination: file_system.durable_path_key(path)?,
                    staged: file_system.staged_file_evidence(&staged)?,
                };
                PreparedEntry {
                    mutation: PreparedMutation::Create {
                        destination: path.clone(),
                        staged,
                    },
                    journal,
                }
            }
            PlannedMutation::Update {
                path,
                before,
                contents,
                ..
            } => {
                let expected = FileEvidence::from(before);
                let staged =
                    stage(file_system, storage, path, contents, Some(&before.metadata)).await?;
                let backup = backup(file_system, storage, path, &expected).await?;
                let journal = replacement_journal(file_system, path, &expected, &staged, &backup)?;
                PreparedEntry {
                    mutation: PreparedMutation::Replace {
                        destination: path.clone(),
                        expected,
                        staged,
                        backup,
                    },
                    journal,
                }
            }
            PlannedMutation::Delete { path, before, .. } => {
                let expected = FileEvidence::from(before);
                let backup = backup(file_system, storage, path, &expected).await?;
                let journal = JournalOperation::Delete {
                    path: file_system.durable_path_key(path)?,
                    before: expected.clone(),
                    backup: file_system.backup_evidence(&backup)?,
                };
                PreparedEntry {
                    mutation: PreparedMutation::Remove {
                        source: path.clone(),
                        expected,
                        backup,
                    },
                    journal,
                }
            }
            PlannedMutation::Move {
                source,
                before,
                destination,
                contents,
                ..
            } => {
                let expected = FileEvidence::from(before);
                let staged = stage(
                    file_system,
                    storage,
                    destination,
                    contents,
                    Some(&before.metadata),
                )
                .await?;
                let backup = backup(file_system, storage, source, &expected).await?;
                let journal = JournalOperation::Move {
                    source: file_system.durable_path_key(source)?,
                    destination: file_system.durable_path_key(destination)?,
                    before: expected.clone(),
                    staged: file_system.staged_file_evidence(&staged)?,
                    backup: file_system.backup_evidence(&backup)?,
                };
                PreparedEntry {
                    mutation: PreparedMutation::Move {
                        source: source.clone(),
                        expected,
                        destination: destination.clone(),
                        staged,
                        backup,
                    },
                    journal,
                }
            }
        };
        prepared.push(entry);
    }
    file_system.sync_storage(storage).await?;
    Ok(prepared)
}

async fn stage<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    destination: &F::ResolvedPath,
    contents: &[u8],
    metadata: Option<&crate::MetadataSnapshot>,
) -> Result<F::StagedFile, TransactionFileSystemError> {
    let staged = file_system
        .stage_file(
            storage,
            StageFileRequest {
                destination,
                contents,
                metadata,
            },
        )
        .await?;
    file_system.sync_staged_file(&staged).await?;
    Ok(staged)
}

async fn backup<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    source: &F::ResolvedPath,
    expected: &FileEvidence,
) -> Result<F::Backup, TransactionFileSystemError> {
    let backup = file_system.backup_file(storage, source, expected).await?;
    file_system.sync_backup(&backup).await?;
    Ok(backup)
}

fn replacement_journal<F: TransactionFileSystem>(
    file_system: &F,
    path: &F::ResolvedPath,
    before: &FileEvidence,
    staged: &F::StagedFile,
    backup: &F::Backup,
) -> Result<JournalOperation, TransactionFileSystemError> {
    Ok(JournalOperation::Update {
        path: file_system.durable_path_key(path)?,
        before: before.clone(),
        staged: file_system.staged_file_evidence(staged)?,
        backup: file_system.backup_evidence(backup)?,
    })
}
