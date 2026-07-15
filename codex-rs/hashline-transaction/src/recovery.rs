use std::collections::BTreeSet;

use thiserror::Error;

use crate::DurableTransactionKey;
use crate::ExactBytesDigest;
use crate::JournalError;
use crate::JournalReadLimits;
use crate::JournalRecord;
use crate::JournalState;
use crate::RecoveryScanLimit;
use crate::RecoveryTarget;
use crate::TransactionFileSystem;
use crate::TransactionFileSystemError;
use crate::TransactionId;
use crate::TransactionLimits;
use crate::executor::ExecutionFailure;
use crate::executor::transition_and_persist;
use crate::recovered::RecoveredEntry;
use crate::recovered::ordered_lock_paths;
use crate::recovered::reopen_entries;
use crate::recovery_rollback::converge_rollback;
use crate::recovery_rollback::rollback_is_terminal;
use crate::recovery_verify::verify_committed;
use crate::recovery_verify::verify_rolled_back;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryResult {
    pub transaction_id: TransactionId,
    pub plan_digest: ExactBytesDigest,
    pub outcome: RecoveryOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryOutcome {
    Committed,
    RolledBack,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryAttempt {
    pub key: DurableTransactionKey,
    pub result: Result<RecoveryResult, RecoveryError>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RecoveryFailure {
    #[error(transparent)]
    FileSystem(#[from] TransactionFileSystemError),
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error("recovery scan returned {observed} transactions, exceeding the limit {limit}")]
    ScanLimit { observed: u64, limit: u64 },
    #[error("durable transaction key is invalid: {reason}")]
    InvalidTransactionKey { reason: &'static str },
    #[error("durable transaction key does not match the loaded journal")]
    TransactionKeyMismatch,
    #[error("journal environment does not match the recovery capability")]
    EnvironmentMismatch,
    #[error("journal manifest changed while recovery waited for path locks")]
    ManifestMismatch,
    #[error("transaction root identity changed before recovery could lock it")]
    RootIdentityMismatch,
}

enum ConvergenceFailure {
    Unavailable(RecoveryFailure),
    Current(RecoveryFailure),
}

impl From<ExecutionFailure> for RecoveryFailure {
    fn from(failure: ExecutionFailure) -> Self {
        match failure {
            ExecutionFailure::FileSystem(error) => Self::FileSystem(error),
            ExecutionFailure::Journal(error) => Self::Journal(error),
        }
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RecoveryError {
    #[error("transaction recovery could not load a trusted journal: {failure}")]
    Unavailable { failure: RecoveryFailure },
    #[error("transaction {transaction_id:?} requires recovery: {failure}")]
    RecoveryRequired {
        transaction_id: TransactionId,
        failure: RecoveryFailure,
        record_failure: Option<RecoveryFailure>,
    },
}

pub async fn recover_pending<F: TransactionFileSystem>(
    file_system: &F,
    scan_limit: RecoveryScanLimit,
    limits: TransactionLimits,
) -> Result<Vec<RecoveryAttempt>, RecoveryFailure> {
    let keys = file_system.pending_recovery(scan_limit).await?;
    if keys.len() as u64 > scan_limit.max_transactions {
        return Err(RecoveryFailure::ScanLimit {
            observed: keys.len() as u64,
            limit: scan_limit.max_transactions,
        });
    }
    let mut attempts = Vec::with_capacity(keys.len());
    let mut seen = BTreeSet::new();
    for key in keys {
        let validation = validate_transaction_key(&key, limits).and_then(|()| {
            if seen.insert(key.clone()) {
                Ok(())
            } else {
                Err(RecoveryFailure::InvalidTransactionKey {
                    reason: "recovery scan returned a duplicate key",
                })
            }
        });
        let result = match validation {
            Ok(()) => recover_transaction(file_system, &key, limits).await,
            Err(failure) => Err(RecoveryError::Unavailable { failure }),
        };
        attempts.push(RecoveryAttempt { key, result });
    }
    Ok(attempts)
}

pub async fn recover_transaction<F: TransactionFileSystem>(
    file_system: &F,
    key: &DurableTransactionKey,
    limits: TransactionLimits,
) -> Result<RecoveryResult, RecoveryError> {
    let record_result = load_record(file_system, key, limits).await;
    let (storage, mut journal, mut record) = match record_result {
        Ok(record) => record,
        Err(failure) => {
            return Err(RecoveryError::Unavailable { failure });
        }
    };
    let transaction_id = record.transaction_id.clone();
    let plan_digest = record.plan_digest;
    let convergence = converge(file_system, &storage, &mut journal, &mut record, limits).await;
    match convergence {
        Ok(outcome) => Ok(RecoveryResult {
            transaction_id,
            plan_digest,
            outcome,
        }),
        Err(ConvergenceFailure::Unavailable(failure)) => {
            Err(RecoveryError::Unavailable { failure })
        }
        Err(ConvergenceFailure::Current(failure)) => {
            let record_failure = mark_recovery_required(file_system, &storage, &mut record, limits)
                .await
                .err();
            Err(RecoveryError::RecoveryRequired {
                transaction_id,
                failure,
                record_failure,
            })
        }
    }
}

async fn load_record<F: TransactionFileSystem>(
    file_system: &F,
    key: &DurableTransactionKey,
    limits: TransactionLimits,
) -> Result<(F::Storage, F::Journal, JournalRecord), RecoveryFailure> {
    validate_transaction_key(key, limits)?;
    let environment_id = file_system.recovery_environment_id()?;
    let storage = file_system.lock_recovery_storage(key).await?;
    let (journal, record) = read_record(file_system, &storage, limits).await?;
    require_record_context(&record, key, &environment_id)?;
    Ok((storage, journal, record))
}

async fn converge<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    journal: &mut F::Journal,
    record: &mut JournalRecord,
    limits: TransactionLimits,
) -> Result<RecoveryOutcome, ConvergenceFailure> {
    let root = file_system
        .reopen_root(&record.root)
        .await
        .map_err(RecoveryFailure::from)
        .map_err(ConvergenceFailure::Unavailable)?;
    require_root_identity(file_system, &root, record).map_err(ConvergenceFailure::Unavailable)?;
    let entries = reopen_entries(file_system, &root, &record.mutations)
        .await
        .map_err(RecoveryFailure::from)
        .map_err(ConvergenceFailure::Unavailable)?;
    let paths = ordered_lock_paths(file_system, &entries)
        .map_err(RecoveryFailure::from)
        .map_err(ConvergenceFailure::Unavailable)?;
    let lease = file_system
        .lock_paths(&root, &paths)
        .await
        .map_err(RecoveryFailure::from)
        .map_err(ConvergenceFailure::Unavailable)?;
    refresh_record(file_system, storage, journal, record, limits)
        .await
        .map_err(ConvergenceFailure::Unavailable)?;
    require_root_identity(file_system, &root, record).map_err(ConvergenceFailure::Current)?;

    let outcome = async {
        match record.recovery_target {
            RecoveryTarget::Commit => {
                verify_committed(file_system, &lease, &entries, limits).await?;
                if record.state != JournalState::Complete {
                    finish_cleanup(file_system, storage, journal, record, limits).await?;
                }
                Ok(RecoveryOutcome::Committed)
            }
            RecoveryTarget::Rollback => {
                converge_to_rollback(
                    file_system,
                    &lease,
                    storage,
                    journal,
                    record,
                    &entries,
                    limits,
                )
                .await?;
                Ok(RecoveryOutcome::RolledBack)
            }
        }
    }
    .await;
    outcome.map_err(ConvergenceFailure::Current)
}

async fn read_record<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    limits: TransactionLimits,
) -> Result<(F::Journal, JournalRecord), RecoveryFailure> {
    let loaded = file_system
        .load_journal(storage, limits.max_journal_bytes)
        .await?;
    let record = JournalRecord::from_bounded_json(&loaded.bytes, JournalReadLimits::from(limits))?;
    Ok((loaded.journal, record))
}

async fn refresh_record<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    journal: &mut F::Journal,
    record: &mut JournalRecord,
    limits: TransactionLimits,
) -> Result<(), RecoveryFailure> {
    let (latest_journal, latest_record) = read_record(file_system, storage, limits).await?;
    require_record_context(
        &latest_record,
        &record.transaction_key,
        &record.environment_id,
    )?;
    if latest_record.manifest_digest != record.manifest_digest {
        return Err(RecoveryFailure::ManifestMismatch);
    }
    *journal = latest_journal;
    *record = latest_record;
    Ok(())
}

fn require_record_context(
    record: &JournalRecord,
    key: &DurableTransactionKey,
    environment_id: &str,
) -> Result<(), RecoveryFailure> {
    if record.transaction_key != *key {
        return Err(RecoveryFailure::TransactionKeyMismatch);
    }
    if record.environment_id != environment_id {
        return Err(RecoveryFailure::EnvironmentMismatch);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn converge_to_rollback<F: TransactionFileSystem>(
    file_system: &F,
    lease: &F::Lease,
    storage: &F::Storage,
    journal: &mut F::Journal,
    record: &mut JournalRecord,
    entries: &[RecoveredEntry<F::ResolvedPath>],
    limits: TransactionLimits,
) -> Result<(), RecoveryFailure> {
    if record.state == JournalState::Complete {
        verify_rolled_back(file_system, lease, &record.mutations, entries, limits).await?;
        return Ok(());
    }
    if record.state == JournalState::RecoveryRequired && rollback_is_terminal(record) {
        verify_rolled_back(file_system, lease, &record.mutations, entries, limits).await?;
        finish_cleanup(file_system, storage, journal, record, limits).await?;
        return Ok(());
    }
    if record.state != JournalState::RolledBack && record.state != JournalState::Cleaning {
        converge_rollback(
            file_system,
            lease,
            storage,
            record,
            entries,
            journal,
            limits,
        )
        .await?;
    }
    verify_rolled_back(file_system, lease, &record.mutations, entries, limits).await?;
    finish_cleanup(file_system, storage, journal, record, limits).await
}

async fn finish_cleanup<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    journal: &mut F::Journal,
    record: &mut JournalRecord,
    limits: TransactionLimits,
) -> Result<(), RecoveryFailure> {
    if record.state != JournalState::Cleaning {
        *journal =
            transition_and_persist(file_system, storage, record, JournalState::Cleaning, limits)
                .await?;
    }
    file_system.cleanup_artifacts(storage).await?;
    *journal = transition_and_persist(file_system, storage, record, JournalState::Complete, limits)
        .await?;
    Ok(())
}

async fn mark_recovery_required<F: TransactionFileSystem>(
    file_system: &F,
    storage: &F::Storage,
    record: &mut JournalRecord,
    limits: TransactionLimits,
) -> Result<(), RecoveryFailure> {
    if record.state != JournalState::RecoveryRequired {
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

fn require_root_identity<F: TransactionFileSystem>(
    file_system: &F,
    root: &F::Root,
    record: &JournalRecord,
) -> Result<(), RecoveryFailure> {
    if file_system.root_identity(root)? == record.root_identity {
        Ok(())
    } else {
        Err(RecoveryFailure::RootIdentityMismatch)
    }
}

fn validate_transaction_key(
    key: &DurableTransactionKey,
    limits: TransactionLimits,
) -> Result<(), RecoveryFailure> {
    if key.namespace.is_empty() || key.value.is_empty() {
        return Err(RecoveryFailure::InvalidTransactionKey {
            reason: "namespace and value must not be empty",
        });
    }
    let observed = (key.namespace.len() as u64).saturating_add(key.value.len() as u64);
    if observed > limits.max_executor_key_bytes {
        return Err(RecoveryFailure::InvalidTransactionKey {
            reason: "key exceeds the executor-key byte limit",
        });
    }
    Ok(())
}
