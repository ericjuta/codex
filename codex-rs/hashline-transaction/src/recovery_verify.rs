use crate::FileEvidence;
use crate::JournalMutation;
use crate::JournalOperation;
use crate::MutationProgress;
use crate::ObservationLimit;
use crate::ObservedEvidence;
use crate::TransactionFileSystem;
use crate::TransactionFileSystemError;
use crate::TransactionLimits;
use crate::recovered::RecoveredEntry;
use crate::recovered::RecoveredPaths;

pub(crate) async fn verify_committed<F: TransactionFileSystem>(
    file_system: &F,
    lease: &F::Lease,
    entries: &[RecoveredEntry<F::ResolvedPath>],
    limits: TransactionLimits,
) -> Result<(), TransactionFileSystemError> {
    for entry in entries {
        match (&entry.paths, &entry.operation) {
            (RecoveredPaths::Create { destination }, JournalOperation::Create { staged, .. }) => {
                require_present(file_system, lease, destination, &staged.evidence, limits).await?;
            }
            (RecoveredPaths::Update { path }, JournalOperation::Update { staged, .. }) => {
                require_present(file_system, lease, path, &staged.evidence, limits).await?;
            }
            (RecoveredPaths::Delete { path }, JournalOperation::Delete { .. }) => {
                require_absent(file_system, lease, path, limits).await?
            }
            (
                RecoveredPaths::Move {
                    source,
                    destination,
                },
                JournalOperation::Move { staged, .. },
            ) => {
                require_absent(file_system, lease, source, limits).await?;
                require_present(file_system, lease, destination, &staged.evidence, limits).await?;
            }
            _ => return Err(inconsistent_entry()),
        }
    }
    Ok(())
}

pub(crate) async fn verify_rolled_back<F: TransactionFileSystem>(
    file_system: &F,
    lease: &F::Lease,
    mutations: &[JournalMutation],
    entries: &[RecoveredEntry<F::ResolvedPath>],
    limits: TransactionLimits,
) -> Result<(), TransactionFileSystemError> {
    if mutations.len() != entries.len() {
        return Err(inconsistent_entry());
    }
    for (mutation, entry) in mutations.iter().zip(entries) {
        match (&entry.paths, &entry.operation) {
            (RecoveredPaths::Create { destination }, JournalOperation::Create { .. }) => {
                require_absent(file_system, lease, destination, limits).await?
            }
            (RecoveredPaths::Update { path }, JournalOperation::Update { before, backup, .. })
            | (RecoveredPaths::Delete { path }, JournalOperation::Delete { before, backup, .. }) => {
                let (expected, alternate) =
                    rollback_evidence(mutation.progress, before, &backup.evidence)?;
                require_present_any(file_system, lease, path, expected, alternate, limits).await?;
            }
            (
                RecoveredPaths::Move {
                    source,
                    destination,
                },
                JournalOperation::Move { before, backup, .. },
            ) => {
                let (expected, alternate) =
                    rollback_evidence(mutation.progress, before, &backup.evidence)?;
                require_present_any(file_system, lease, source, expected, alternate, limits)
                    .await?;
                require_absent(file_system, lease, destination, limits).await?;
            }
            _ => return Err(inconsistent_entry()),
        }
    }
    Ok(())
}

fn rollback_evidence<'a>(
    progress: MutationProgress,
    before: &'a FileEvidence,
    backup: &'a FileEvidence,
) -> Result<(&'a FileEvidence, Option<&'a FileEvidence>), TransactionFileSystemError> {
    match progress {
        MutationProgress::Pending => Ok((before, None)),
        MutationProgress::RolledBack => Ok((before, Some(backup))),
        MutationProgress::Committing
        | MutationProgress::Applied
        | MutationProgress::RollingBack => Err(TransactionFileSystemError::Platform {
            operation: "verify recovered rollback",
            reason: "rollback verification began before mutation convergence".to_string(),
        }),
    }
}

async fn require_present<F: TransactionFileSystem>(
    file_system: &F,
    lease: &F::Lease,
    path: &F::ResolvedPath,
    expected: &FileEvidence,
    limits: TransactionLimits,
) -> Result<(), TransactionFileSystemError> {
    require_present_any(file_system, lease, path, expected, None, limits).await
}

async fn require_present_any<F: TransactionFileSystem>(
    file_system: &F,
    lease: &F::Lease,
    path: &F::ResolvedPath,
    expected: &FileEvidence,
    alternate: Option<&FileEvidence>,
    limits: TransactionLimits,
) -> Result<(), TransactionFileSystemError> {
    let observed = file_system
        .observe_evidence_locked(
            lease,
            path,
            ObservationLimit {
                max_bytes: limits.max_file_bytes,
            },
        )
        .await?;
    if observed == ObservedEvidence::Present(expected.clone())
        || alternate
            .is_some_and(|alternate| observed == ObservedEvidence::Present(alternate.clone()))
    {
        Ok(())
    } else {
        Err(disturbed())
    }
}

async fn require_absent<F: TransactionFileSystem>(
    file_system: &F,
    lease: &F::Lease,
    path: &F::ResolvedPath,
    limits: TransactionLimits,
) -> Result<(), TransactionFileSystemError> {
    let observed = file_system
        .observe_evidence_locked(
            lease,
            path,
            ObservationLimit {
                max_bytes: limits.max_file_bytes,
            },
        )
        .await?;
    if observed == ObservedEvidence::Absent {
        Ok(())
    } else {
        Err(disturbed())
    }
}

fn disturbed() -> TransactionFileSystemError {
    TransactionFileSystemError::ChangedSincePlanning {
        path: "recovered transaction path".to_string(),
    }
}

fn inconsistent_entry() -> TransactionFileSystemError {
    TransactionFileSystemError::Platform {
        operation: "verify recovered transaction",
        reason: "journal operation and reopened paths disagree".to_string(),
    }
}
