use thiserror::Error;

use crate::CanonicalPathKey;
use crate::ExactBytesDigest;
use crate::ExecuteError::BeforeCommit;
use crate::JournalError;
use crate::JournalRecord;
use crate::JournalState;
use crate::MutationOutcome;
use crate::MutationProgress;
use crate::ObservationLimit;
use crate::ObservedPath;
use crate::PlannedMutation;
use crate::PlannedTransaction;
use crate::StorageRequirements;
use crate::TransactionAction;
use crate::TransactionFileSystem;
use crate::TransactionFileSystemError;
use crate::TransactionId;
use crate::TransactionLimits;
use crate::prepared::PreparedEntry;
use crate::prepared::prepare;
use crate::rollback::finish;
use crate::rollback::rollback;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionResult {
    pub transaction_id: TransactionId,
    pub plan_digest: ExactBytesDigest,
    pub outcome: ExecutionOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionOutcome {
    Committed,
    RolledBack { failure: ExecutionFailure },
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ExecutionFailure {
    #[error(transparent)]
    FileSystem(#[from] TransactionFileSystemError),
    #[error(transparent)]
    Journal(#[from] JournalError),
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ExecuteError {
    #[error("a preview-only transaction plan cannot be executed")]
    PreviewPlan,
    #[error("transaction failed before durable commit state: {failure}")]
    BeforeCommit { failure: ExecutionFailure },
    #[error(
        "transaction {transaction_id:?} requires recovery after {failure}; recovery failed: {recovery_failure}"
    )]
    RecoveryRequired {
        transaction_id: TransactionId,
        plan_digest: ExactBytesDigest,
        failure: ExecutionFailure,
        recovery_failure: ExecutionFailure,
    },
}

pub async fn execute<F: TransactionFileSystem>(
    file_system: &F,
    mut plan: PlannedTransaction<F::Root, F::ResolvedPath>,
    transaction_id: TransactionId,
    limits: TransactionLimits,
) -> Result<ExecutionResult, ExecuteError> {
    if plan.action == TransactionAction::Preview {
        return Err(ExecuteError::PreviewPlan);
    }
    sort_mutations(&mut plan.mutations);
    let lock_paths = ordered_lock_paths(&plan.mutations);
    let lease = file_system
        .lock_paths(&plan.root, &lock_paths)
        .await
        .map_err(before_commit)?;
    revalidate(file_system, &lease, &plan, limits).await?;
    let root = file_system
        .durable_root_key(&plan.root)
        .map_err(before_commit)?;

    let requirements = StorageRequirements {
        staged_bytes: plan.summary.after_bytes,
        backup_bytes: plan.summary.before_bytes,
        journal_bytes: limits.max_journal_bytes,
    };
    let storage = file_system
        .allocate_storage(&lease, &transaction_id, requirements)
        .await
        .map_err(before_commit)?;
    let transaction_key = file_system
        .durable_transaction_key(&storage)
        .map_err(before_commit)?;
    let prepared = match prepare(file_system, &storage, &plan.mutations).await {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = file_system.cleanup_artifacts(&storage).await;
            return Err(before_commit(error));
        }
    };

    let operations = prepared.iter().map(|entry| entry.journal.clone()).collect();
    let mut record = JournalRecord::new(
        transaction_id.clone(),
        transaction_key,
        plan.environment_id,
        root,
        plan.root_identity,
        plan.plan_digest,
        operations,
    );
    let mut journal = match persist(file_system, &storage, &record, limits).await {
        Ok(journal) => journal,
        Err(failure) => {
            let _ = file_system.cleanup_artifacts(&storage).await;
            return Err(BeforeCommit { failure });
        }
    };

    let commit_result = async {
        journal = transition_and_persist(
            file_system,
            &storage,
            &mut record,
            JournalState::Prepared,
            limits,
        )
        .await?;
        journal = transition_and_persist(
            file_system,
            &storage,
            &mut record,
            JournalState::Committing,
            limits,
        )
        .await?;
        for (index, entry) in prepared.iter().enumerate() {
            commit_one(
                file_system,
                &lease,
                &storage,
                &mut record,
                entry,
                index,
                &mut journal,
                limits,
            )
            .await?;
        }
        journal = transition_and_persist(
            file_system,
            &storage,
            &mut record,
            JournalState::Committed,
            limits,
        )
        .await?;
        Ok::<(), ExecutionFailure>(())
    }
    .await;
    if let Err(failure) = commit_result {
        return rollback(
            file_system,
            &lease,
            &storage,
            &mut record,
            &prepared,
            &mut journal,
            failure,
            limits,
        )
        .await;
    }
    finish(
        file_system,
        &storage,
        &mut record,
        transaction_id,
        plan.plan_digest,
        ExecutionOutcome::Committed,
        limits,
    )
    .await
}

fn before_commit(error: impl Into<ExecutionFailure>) -> ExecuteError {
    BeforeCommit {
        failure: error.into(),
    }
}

fn sort_mutations<P>(mutations: &mut [PlannedMutation<P>]) {
    mutations.sort_by(|left, right| mutation_key(left).cmp(mutation_key(right)));
}

fn mutation_key<P>(mutation: &PlannedMutation<P>) -> &CanonicalPathKey {
    match mutation {
        PlannedMutation::Create { path_key, .. }
        | PlannedMutation::Update { path_key, .. }
        | PlannedMutation::Delete { path_key, .. } => path_key,
        PlannedMutation::Move { source_key, .. } => source_key,
    }
}

fn ordered_lock_paths<P: Clone>(mutations: &[PlannedMutation<P>]) -> Vec<P> {
    let mut paths = Vec::with_capacity(mutations.len().saturating_mul(/*rhs*/ 2));
    for mutation in mutations {
        match mutation {
            PlannedMutation::Create { path, path_key, .. }
            | PlannedMutation::Update { path, path_key, .. }
            | PlannedMutation::Delete { path, path_key, .. } => {
                paths.push((path_key, path.clone()));
            }
            PlannedMutation::Move {
                source,
                source_key,
                destination,
                destination_key,
                ..
            } => {
                paths.push((source_key, source.clone()));
                paths.push((destination_key, destination.clone()));
            }
        }
    }
    paths.sort_by_key(|(left, _)| *left);
    paths.into_iter().map(|(_, path)| path).collect()
}

async fn revalidate<F: TransactionFileSystem>(
    file_system: &F,
    lease: &F::Lease,
    plan: &PlannedTransaction<F::Root, F::ResolvedPath>,
    limits: TransactionLimits,
) -> Result<(), ExecuteError> {
    if file_system
        .root_identity(&plan.root)
        .map_err(before_commit)?
        != plan.root_identity
    {
        return Err(before_commit(
            TransactionFileSystemError::ChangedSincePlanning {
                path: ".".to_string(),
            },
        ));
    }
    for mutation in &plan.mutations {
        match mutation {
            PlannedMutation::Create {
                path, model_path, ..
            } => require_absent(file_system, lease, path, model_path, limits).await?,
            PlannedMutation::Update {
                path,
                model_path,
                before,
                ..
            }
            | PlannedMutation::Delete {
                path,
                model_path,
                before,
                ..
            } => require_unchanged(file_system, lease, path, model_path, before, limits).await?,
            PlannedMutation::Move {
                source,
                model_source,
                before,
                destination,
                model_destination,
                ..
            } => {
                require_unchanged(file_system, lease, source, model_source, before, limits).await?;
                require_absent(file_system, lease, destination, model_destination, limits).await?;
            }
        }
    }
    Ok(())
}

async fn require_absent<F: TransactionFileSystem>(
    file_system: &F,
    lease: &F::Lease,
    path: &F::ResolvedPath,
    model_path: &str,
    limits: TransactionLimits,
) -> Result<(), ExecuteError> {
    let observed = file_system
        .reobserve_locked(
            lease,
            path,
            ObservationLimit {
                max_bytes: limits.max_file_bytes,
            },
        )
        .await
        .map_err(before_commit)?;
    if observed != ObservedPath::Absent {
        return Err(before_commit(
            TransactionFileSystemError::ChangedSincePlanning {
                path: model_path.to_string(),
            },
        ));
    }
    Ok(())
}

async fn require_unchanged<F: TransactionFileSystem>(
    file_system: &F,
    lease: &F::Lease,
    path: &F::ResolvedPath,
    model_path: &str,
    expected: &crate::ObservedFile,
    limits: TransactionLimits,
) -> Result<(), ExecuteError> {
    let observed = file_system
        .reobserve_locked(
            lease,
            path,
            ObservationLimit {
                max_bytes: limits.max_file_bytes,
            },
        )
        .await
        .map_err(before_commit)?;
    if observed != ObservedPath::Present(expected.clone()) {
        return Err(before_commit(
            TransactionFileSystemError::ChangedSincePlanning {
                path: model_path.to_string(),
            },
        ));
    }
    Ok(())
}

pub(crate) async fn persist<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    record: &JournalRecord,
    limits: TransactionLimits,
) -> Result<F::Journal, ExecutionFailure> {
    let bytes = record.to_bounded_json(limits.max_journal_bytes)?;
    let journal = file_system.persist_journal(storage, &bytes).await?;
    file_system.sync_journal(&journal).await?;
    file_system.sync_storage(storage).await?;
    Ok(journal)
}

pub(crate) async fn transition_and_persist<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    record: &mut JournalRecord,
    state: JournalState,
    limits: TransactionLimits,
) -> Result<F::Journal, ExecutionFailure> {
    let mut next = record.clone();
    next.transition(state)?;
    let journal = persist(file_system, storage, &next, limits).await?;
    *record = next;
    Ok(journal)
}

pub(crate) async fn progress_and_persist<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    record: &mut JournalRecord,
    index: usize,
    progress: MutationProgress,
    limits: TransactionLimits,
) -> Result<F::Journal, ExecutionFailure> {
    let mut next = record.clone();
    next.set_mutation_progress(index, progress)?;
    let journal = persist(file_system, storage, &next, limits).await?;
    *record = next;
    Ok(journal)
}

#[allow(clippy::too_many_arguments)]
async fn commit_one<F: TransactionFileSystem>(
    file_system: &F,
    lease: &F::Lease,
    storage: &F::Storage,
    record: &mut JournalRecord,
    entry: &PreparedEntry<F::ResolvedPath, F::StagedFile, F::Backup>,
    index: usize,
    journal: &mut F::Journal,
    limits: TransactionLimits,
) -> Result<(), ExecutionFailure> {
    *journal = progress_and_persist(
        file_system,
        storage,
        record,
        index,
        MutationProgress::Committing,
        limits,
    )
    .await?;
    let outcome = file_system
        .apply_guarded(lease, journal, entry.mutation.guarded())
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
        MutationProgress::Applied,
        limits,
    )
    .await?;
    Ok(())
}
