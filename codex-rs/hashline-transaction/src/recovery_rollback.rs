use crate::JournalRecord;
use crate::JournalState;
use crate::MutationOutcome;
use crate::MutationProgress;
use crate::TransactionFileSystem;
use crate::TransactionLimits;
use crate::executor::ExecutionFailure;
use crate::executor::progress_and_persist;
use crate::executor::transition_and_persist;
use crate::recovered::RecoveredEntry;
use crate::recovered::reopen_rollback;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn converge_rollback<F: TransactionFileSystem>(
    file_system: &F,
    lease: &F::Lease,
    storage: &F::Storage,
    record: &mut JournalRecord,
    entries: &[RecoveredEntry<F::ResolvedPath>],
    journal: &mut F::Journal,
    limits: TransactionLimits,
) -> Result<(), ExecutionFailure> {
    if entries.len() != record.mutations.len() {
        return Err(crate::TransactionFileSystemError::Platform {
            operation: "recover rollback",
            reason: "journal mutation and reopened entry counts disagree".to_string(),
        }
        .into());
    }
    if record.state != JournalState::RollingBack {
        *journal = transition_and_persist(
            file_system,
            storage,
            record,
            JournalState::RollingBack,
            limits,
        )
        .await?;
    }
    for (index, entry) in entries.iter().enumerate().rev() {
        match record.mutations[index].progress {
            MutationProgress::Pending | MutationProgress::RolledBack => continue,
            MutationProgress::Committing | MutationProgress::Applied => {
                *journal = progress_and_persist(
                    file_system,
                    storage,
                    record,
                    index,
                    MutationProgress::RollingBack,
                    limits,
                )
                .await?;
            }
            MutationProgress::RollingBack => {}
        }
        let mutation = reopen_rollback(file_system, storage, entry).await?;
        let outcome = file_system
            .restore_guarded(lease, journal, mutation.rollback())
            .await?;
        match outcome {
            MutationOutcome::Applied | MutationOutcome::AlreadyApplied => {}
        }
        for path in mutation.parent_paths() {
            file_system.sync_parent(path).await?;
        }
        *journal = progress_and_persist(
            file_system,
            storage,
            record,
            index,
            MutationProgress::RolledBack,
            limits,
        )
        .await?;
    }
    *journal = transition_and_persist(
        file_system,
        storage,
        record,
        JournalState::RolledBack,
        limits,
    )
    .await?;
    Ok(())
}

pub(crate) fn rollback_is_terminal(record: &JournalRecord) -> bool {
    record.mutations.iter().all(|mutation| {
        matches!(
            mutation.progress,
            MutationProgress::Pending | MutationProgress::RolledBack
        )
    })
}
