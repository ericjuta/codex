use std::io;
use std::io::Write;
use std::num::NonZeroU64;

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

use crate::DurablePathKey;
use crate::ExactBytesDigest;
use crate::ExecutorFileIdentity;
use crate::ExecutorRootIdentity;
use crate::FileKind;
use crate::MetadataSnapshot;
use crate::ObservedFile;
use crate::TransactionId;

pub const TRANSACTION_JOURNAL_SCHEMA_VERSION: u32 = 1;

/// Exact identity and metadata evidence persisted without retaining file contents.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEvidence {
    pub exact_digest: ExactBytesDigest,
    pub identity: ExecutorFileIdentity,
    pub metadata: MetadataSnapshot,
    pub link_count: NonZeroU64,
    pub kind: FileKind,
}

impl From<&ObservedFile> for FileEvidence {
    fn from(file: &ObservedFile) -> Self {
        Self {
            exact_digest: file.exact_digest,
            identity: file.identity.clone(),
            metadata: file.metadata.clone(),
            link_count: file.link_count,
            kind: file.kind,
        }
    }
}

/// Durable key and evidence for one staged after-image or rollback backup.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableFileEvidence {
    pub key: DurablePathKey,
    pub evidence: FileEvidence,
}

/// Serialized journal bytes that have already passed the configured hard cap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalBytes(Vec<u8>);

impl JournalBytes {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Storage capacity reserved before the journal can become prepared.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StorageRequirements {
    pub staged_bytes: u64,
    pub backup_bytes: u64,
    pub journal_bytes: u64,
}

impl StorageRequirements {
    pub fn total_bytes(self) -> u64 {
        self.staged_bytes
            .saturating_add(self.backup_bytes)
            .saturating_add(self.journal_bytes)
    }
}

/// Durable phase for the transaction as a whole.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JournalState {
    Preparing,
    Prepared,
    Committing,
    Committed,
    RollingBack,
    RolledBack,
    Cleaning,
    Complete,
    RecoveryRequired,
}

/// Durable progress for one ordered mutation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MutationProgress {
    Pending,
    Committing,
    Applied,
    RollingBack,
    RolledBack,
}

/// Recovery evidence for one planned mutation without full before/after contents.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum JournalOperation {
    Create {
        destination: DurablePathKey,
        staged: DurableFileEvidence,
    },
    Update {
        path: DurablePathKey,
        before: FileEvidence,
        staged: DurableFileEvidence,
        backup: DurableFileEvidence,
    },
    Delete {
        path: DurablePathKey,
        before: FileEvidence,
        backup: DurableFileEvidence,
    },
    Move {
        source: DurablePathKey,
        destination: DurablePathKey,
        before: FileEvidence,
        staged: DurableFileEvidence,
        backup: DurableFileEvidence,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalMutation {
    pub progress: MutationProgress,
    pub operation: JournalOperation,
}

/// Versioned, bounded durable record used by commit and restart recovery.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalRecord {
    pub schema_version: u32,
    pub transaction_id: TransactionId,
    pub environment_id: String,
    pub root: DurablePathKey,
    pub root_identity: ExecutorRootIdentity,
    pub plan_digest: ExactBytesDigest,
    pub state: JournalState,
    pub mutations: Vec<JournalMutation>,
}

impl JournalRecord {
    pub fn new(
        transaction_id: TransactionId,
        environment_id: String,
        root: DurablePathKey,
        root_identity: ExecutorRootIdentity,
        plan_digest: ExactBytesDigest,
        operations: Vec<JournalOperation>,
    ) -> Self {
        Self {
            schema_version: TRANSACTION_JOURNAL_SCHEMA_VERSION,
            transaction_id,
            environment_id,
            root,
            root_identity,
            plan_digest,
            state: JournalState::Preparing,
            mutations: operations
                .into_iter()
                .map(|operation| JournalMutation {
                    progress: MutationProgress::Pending,
                    operation,
                })
                .collect(),
        }
    }

    pub fn transition(&mut self, next: JournalState) -> Result<(), JournalError> {
        if !valid_transition(self.state, next) {
            return Err(JournalError::InvalidStateTransition {
                current: self.state,
                next,
            });
        }
        validate_progress(next, &self.mutations)?;
        self.state = next;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), JournalError> {
        if self.schema_version != TRANSACTION_JOURNAL_SCHEMA_VERSION {
            return Err(JournalError::UnsupportedSchema {
                version: self.schema_version,
            });
        }
        validate_progress(self.state, &self.mutations)
    }

    pub fn set_mutation_progress(
        &mut self,
        index: usize,
        next: MutationProgress,
    ) -> Result<(), JournalError> {
        let mutation = self
            .mutations
            .get_mut(index)
            .ok_or(JournalError::MutationIndex { index })?;
        if !valid_progress_transition(self.state, mutation.progress, next) {
            return Err(JournalError::InvalidProgressTransition {
                state: self.state,
                current: mutation.progress,
                next,
            });
        }
        mutation.progress = next;
        Ok(())
    }

    pub fn to_bounded_json(&self, max_bytes: u64) -> Result<JournalBytes, JournalError> {
        self.validate()?;
        let mut counter = ByteCounter::default();
        serde_json::to_writer(&mut counter, self).map_err(serialization_error)?;
        if counter.bytes > max_bytes {
            return Err(JournalError::Limit {
                observed: counter.bytes,
                limit: max_bytes,
            });
        }
        let capacity = usize::try_from(counter.bytes).map_err(|_| JournalError::Limit {
            observed: counter.bytes,
            limit: max_bytes,
        })?;
        let mut bytes = Vec::with_capacity(capacity);
        serde_json::to_writer(&mut bytes, self).map_err(serialization_error)?;
        Ok(JournalBytes(bytes))
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum JournalError {
    #[error("unsupported transaction journal schema version {version}")]
    UnsupportedSchema { version: u32 },
    #[error("invalid journal state transition from {current:?} to {next:?}")]
    InvalidStateTransition {
        current: JournalState,
        next: JournalState,
    },
    #[error("journal state {state:?} is inconsistent with mutation progress")]
    InconsistentProgress { state: JournalState },
    #[error("journal mutation index {index} is out of bounds")]
    MutationIndex { index: usize },
    #[error("invalid mutation progress transition in {state:?} from {current:?} to {next:?}")]
    InvalidProgressTransition {
        state: JournalState,
        current: MutationProgress,
        next: MutationProgress,
    },
    #[error("transaction journal serialization failed: {reason}")]
    Serialization { reason: String },
    #[error("transaction journal byte limit exceeded: observed {observed}, limit {limit}")]
    Limit { observed: u64, limit: u64 },
}

fn serialization_error(error: serde_json::Error) -> JournalError {
    JournalError::Serialization {
        reason: error.to_string(),
    }
}

#[derive(Default)]
struct ByteCounter {
    bytes: u64,
}

impl Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buffer.len() as u64);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn valid_transition(current: JournalState, next: JournalState) -> bool {
    matches!(
        (current, next),
        (JournalState::Preparing, JournalState::Prepared)
            | (JournalState::Preparing, JournalState::RecoveryRequired)
            | (JournalState::Prepared, JournalState::Committing)
            | (JournalState::Prepared, JournalState::RollingBack)
            | (JournalState::Prepared, JournalState::RecoveryRequired)
            | (JournalState::Committing, JournalState::Committed)
            | (JournalState::Committing, JournalState::RollingBack)
            | (JournalState::Committing, JournalState::RecoveryRequired)
            | (JournalState::Committed, JournalState::Cleaning)
            | (JournalState::Committed, JournalState::RecoveryRequired)
            | (JournalState::RollingBack, JournalState::RolledBack)
            | (JournalState::RollingBack, JournalState::RecoveryRequired)
            | (JournalState::RolledBack, JournalState::Cleaning)
            | (JournalState::RolledBack, JournalState::RecoveryRequired)
            | (JournalState::Cleaning, JournalState::Complete)
            | (JournalState::Cleaning, JournalState::RecoveryRequired)
    )
}

fn validate_progress(
    state: JournalState,
    mutations: &[JournalMutation],
) -> Result<(), JournalError> {
    let consistent = match state {
        JournalState::Preparing | JournalState::Prepared => mutations
            .iter()
            .all(|mutation| mutation.progress == MutationProgress::Pending),
        JournalState::Committing => mutations.iter().all(|mutation| {
            matches!(
                mutation.progress,
                MutationProgress::Pending
                    | MutationProgress::Committing
                    | MutationProgress::Applied
            )
        }),
        JournalState::Committed => mutations
            .iter()
            .all(|mutation| mutation.progress == MutationProgress::Applied),
        JournalState::RollingBack => mutations.iter().all(|mutation| {
            matches!(
                mutation.progress,
                MutationProgress::Pending
                    | MutationProgress::Committing
                    | MutationProgress::Applied
                    | MutationProgress::RollingBack
                    | MutationProgress::RolledBack
            )
        }),
        JournalState::RolledBack => mutations.iter().all(|mutation| {
            matches!(
                mutation.progress,
                MutationProgress::Pending | MutationProgress::RolledBack
            )
        }),
        JournalState::Cleaning | JournalState::Complete => {
            let all_applied = mutations
                .iter()
                .all(|mutation| mutation.progress == MutationProgress::Applied);
            let all_rolled_back = mutations.iter().all(|mutation| {
                matches!(
                    mutation.progress,
                    MutationProgress::Pending | MutationProgress::RolledBack
                )
            });
            all_applied || all_rolled_back
        }
        JournalState::RecoveryRequired => true,
    };
    if consistent {
        Ok(())
    } else {
        Err(JournalError::InconsistentProgress { state })
    }
}

fn valid_progress_transition(
    state: JournalState,
    current: MutationProgress,
    next: MutationProgress,
) -> bool {
    match state {
        JournalState::Committing => matches!(
            (current, next),
            (MutationProgress::Pending, MutationProgress::Committing)
                | (MutationProgress::Committing, MutationProgress::Applied)
        ),
        JournalState::RollingBack => matches!(
            (current, next),
            (MutationProgress::Applied, MutationProgress::RollingBack)
                | (MutationProgress::Committing, MutationProgress::RollingBack)
                | (MutationProgress::RollingBack, MutationProgress::RolledBack)
        ),
        _ => false,
    }
}
