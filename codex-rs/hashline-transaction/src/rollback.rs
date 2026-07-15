use crate::ExactBytesDigest;
use crate::JournalRecord;
use crate::JournalState;
use crate::MutationOutcome;
use crate::MutationProgress;
use crate::TransactionFileSystem;
use crate::TransactionFileSystemError;
use crate::TransactionId;
use crate::TransactionLimits;
use crate::executor::ExecuteError;
use crate::executor::ExecutionFailure;
use crate::executor::ExecutionOutcome;
use crate::executor::ExecutionResult;
use crate::executor::persist;
use crate::executor::progress_and_persist;
use crate::executor::transition_and_persist;
use crate::prepared::PreparedEntry;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn rollback<F: TransactionFileSystem>(
    file_system: &F,
    lease: &F::Lease,
    storage: &F::Storage,
    record: &mut JournalRecord,
    prepared: &[PreparedEntry<F::ResolvedPath, F::StagedFile, F::Backup>],
    journal: &mut F::Journal,
    failure: ExecutionFailure,
    limits: TransactionLimits,
) -> Result<ExecutionResult, ExecuteError> {
    let transaction_id = record.transaction_id.clone();
    let plan_digest = record.plan_digest;
    let rollback_result = async {
        *journal = transition_and_persist(
            file_system,
            storage,
            record,
            JournalState::RollingBack,
            limits,
        )
        .await?;
        for (index, entry) in prepared.iter().enumerate().rev() {
            if record.mutations[index].progress == MutationProgress::Pending {
                continue;
            }
            *journal = progress_and_persist(
                file_system,
                storage,
                record,
                index,
                MutationProgress::RollingBack,
                limits,
            )
            .await?;
            let outcome = file_system
                .restore_guarded(lease, journal, entry.mutation.rollback())
                .await?;
            match outcome {
                MutationOutcome::Applied | MutationOutcome::AlreadyApplied => {}
            }
            for path in entry.mutation.parent_paths() {
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
        Ok::<(), ExecutionFailure>(())
    }
    .await;

    if let Err(recovery_failure) = rollback_result {
        let _ = mark_recovery_required(file_system, storage, record, limits).await;
        return Err(ExecuteError::RecoveryRequired {
            transaction_id,
            plan_digest,
            failure,
            recovery_failure,
        });
    }
    finish(
        file_system,
        storage,
        record,
        transaction_id,
        plan_digest,
        ExecutionOutcome::RolledBack { failure },
        limits,
    )
    .await
}

pub(crate) async fn finish<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    record: &mut JournalRecord,
    transaction_id: TransactionId,
    plan_digest: ExactBytesDigest,
    outcome: ExecutionOutcome,
    limits: TransactionLimits,
) -> Result<ExecutionResult, ExecuteError> {
    let result = async {
        transition_and_persist(file_system, storage, record, JournalState::Cleaning, limits)
            .await?;
        file_system.cleanup_artifacts(storage).await?;
        transition_and_persist(file_system, storage, record, JournalState::Complete, limits)
            .await?;
        Ok::<(), ExecutionFailure>(())
    }
    .await;
    if let Err(recovery_failure) = result {
        let failure = ExecutionFailure::FileSystem(TransactionFileSystemError::Platform {
            operation: "finish transaction",
            reason: "terminal transaction cleanup did not complete".to_string(),
        });
        let _ = mark_recovery_required(file_system, storage, record, limits).await;
        return Err(ExecuteError::RecoveryRequired {
            transaction_id,
            plan_digest,
            failure,
            recovery_failure,
        });
    }
    Ok(ExecutionResult {
        transaction_id,
        plan_digest,
        outcome,
    })
}

async fn mark_recovery_required<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    record: &mut JournalRecord,
    limits: TransactionLimits,
) -> Result<(), ExecutionFailure> {
    if record.state == JournalState::RecoveryRequired {
        persist(file_system, storage, record, limits).await?;
    } else {
        transition_and_persist(
            file_system,
            storage,
            record,
            JournalState::RecoveryRequired,
            limits,
        )
        .await?;
    }
    Ok(())
}
