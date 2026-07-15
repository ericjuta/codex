use std::fmt::Debug;
use std::future::Future;

use codex_utils_path_uri::PathUri;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

use crate::MetadataSnapshot;
use crate::ObservationLimit;
use crate::ObservedPath;

/// Error reported by an executor-owned transaction filesystem capability.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum TransactionFileSystemError {
    #[error("transaction filesystem capability `{capability}` is unavailable: {reason}")]
    Unsupported {
        capability: &'static str,
        reason: String,
    },
    #[error("transaction root `{root}` is invalid: {reason}")]
    InvalidRoot { root: PathUri, reason: String },
    #[error("transaction path `{path}` is invalid: {reason}")]
    InvalidModelPath { path: String, reason: String },
    #[error("transaction path `{path}` traverses a symbolic link")]
    SymbolicLink { path: String },
    #[error("transaction path `{path}` changed while planning")]
    ChangedDuringPlanning { path: String },
    #[error("transaction filesystem operation `{operation}` failed: {reason}")]
    Platform {
        operation: &'static str,
        reason: String,
    },
}

/// Read-only filesystem view used while compiling a transaction plan.
///
/// Implementations execute inside the selected environment. They must resolve each
/// model-provided path relative to the opened [`PathUri`] root, reject symbolic links
/// component by component, and retain whatever native handles are needed to bind an
/// observation to the same object during commit. Implementations must enforce
/// [`ObservationLimit`] before retaining file contents in memory.
pub trait PlanningFileSystem: Send + Sync {
    type Root: Clone + Debug + Send + Sync;
    type ResolvedPath: Clone + Debug + Eq + Send + Sync;

    fn open_root(
        &self,
        root: &PathUri,
    ) -> impl Future<Output = Result<Self::Root, TransactionFileSystemError>> + Send;

    fn resolve(
        &self,
        root: &Self::Root,
        model_path: &str,
    ) -> impl Future<Output = Result<Self::ResolvedPath, TransactionFileSystemError>> + Send;

    fn observe(
        &self,
        path: &Self::ResolvedPath,
        limit: ObservationLimit,
    ) -> impl Future<Output = Result<ObservedPath, TransactionFileSystemError>> + Send;

    fn root_identity(
        &self,
        root: &Self::Root,
    ) -> Result<ExecutorRootIdentity, TransactionFileSystemError>;

    fn canonical_path_key(
        &self,
        path: &Self::ResolvedPath,
    ) -> Result<CanonicalPathKey, TransactionFileSystemError>;
}

/// Executor-derived root identity included in deterministic plan digests.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorRootIdentity {
    pub namespace: String,
    pub value: Vec<u8>,
}

/// Executor-derived path key used for conflict detection and deterministic ordering.
///
/// This key is scoped to one opened root and transaction plan. It must not be
/// persisted as recovery state.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalPathKey {
    pub namespace: String,
    pub value: Vec<u8>,
}

/// Executor-native path key that can be reopened safely after a restart.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DurablePathKey {
    pub namespace: String,
    pub value: Vec<u8>,
}

/// Executor-native key that can reopen transaction evidence after a restart.
///
/// The key is meaningful only to the implementation named by `namespace`. It is
/// deliberately not a path or URI and must never be interpreted by the app host.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableTransactionKey {
    pub namespace: String,
    pub value: Vec<u8>,
}

/// Collision-resistant transaction identifier chosen by the transaction engine.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransactionId(pub String);

/// Durable transaction state persisted before the corresponding visible operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JournalState {
    Prepared,
    Applying,
    RollingBack,
    Committed,
    RolledBack,
    RecoveryRequired,
}

/// Request to stage one exact after-image on the destination filesystem.
#[derive(Debug)]
pub struct StageFileRequest<'a, P> {
    pub destination: &'a P,
    pub contents: &'a [u8],
    pub metadata: Option<&'a MetadataSnapshot>,
}

/// One guarded visible filesystem mutation.
#[derive(Debug)]
pub enum GuardedMutation<'a, P, S, B> {
    Create {
        destination: &'a P,
        staged: &'a S,
    },
    Replace {
        destination: &'a P,
        expected: &'a ObservedPath,
        staged: &'a S,
        backup: &'a B,
    },
    Remove {
        source: &'a P,
        expected: &'a ObservedPath,
        backup: &'a B,
    },
    Move {
        source: &'a P,
        expected: &'a ObservedPath,
        destination: &'a P,
        staged: Option<&'a S>,
        backup: &'a B,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationOutcome {
    Applied,
    AlreadyApplied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryOutcome {
    Committed,
    RolledBack,
    RecoveryRequired,
}

/// Locking and final-precondition portion of the transaction capability.
pub trait TransactionCoordination: PlanningFileSystem {
    type Lease: Debug + Send + Sync;

    fn lock_paths(
        &self,
        root: &Self::Root,
        paths: &[Self::ResolvedPath],
    ) -> impl Future<Output = Result<Self::Lease, TransactionFileSystemError>> + Send;

    fn reobserve_locked(
        &self,
        lease: &Self::Lease,
        path: &Self::ResolvedPath,
        limit: ObservationLimit,
    ) -> impl Future<Output = Result<ObservedPath, TransactionFileSystemError>> + Send;
}

/// Staging, backup, journal, and sync portion of the transaction capability.
pub trait TransactionStorage: TransactionCoordination {
    type Storage: Debug + Send + Sync;
    type StagedFile: Debug + Send + Sync;
    type Backup: Debug + Send + Sync;
    type Journal: Debug + Send + Sync;
    type JournalRecord: Debug + Send + Sync;

    fn allocate_storage(
        &self,
        lease: &Self::Lease,
        transaction_id: &TransactionId,
    ) -> impl Future<Output = Result<Self::Storage, TransactionFileSystemError>> + Send;

    fn stage_file(
        &self,
        storage: &Self::Storage,
        request: StageFileRequest<'_, Self::ResolvedPath>,
    ) -> impl Future<Output = Result<Self::StagedFile, TransactionFileSystemError>> + Send;

    fn backup_file(
        &self,
        storage: &Self::Storage,
        source: &Self::ResolvedPath,
        expected: &ObservedPath,
    ) -> impl Future<Output = Result<Self::Backup, TransactionFileSystemError>> + Send;

    fn durable_path_key(
        &self,
        path: &Self::ResolvedPath,
    ) -> Result<DurablePathKey, TransactionFileSystemError>;

    fn persist_journal(
        &self,
        storage: &Self::Storage,
        record: &Self::JournalRecord,
        state: JournalState,
    ) -> impl Future<Output = Result<Self::Journal, TransactionFileSystemError>> + Send;

    fn sync_staged_file(
        &self,
        staged: &Self::StagedFile,
    ) -> impl Future<Output = Result<(), TransactionFileSystemError>> + Send;

    fn sync_journal(
        &self,
        journal: &Self::Journal,
    ) -> impl Future<Output = Result<(), TransactionFileSystemError>> + Send;

    fn cleanup_storage(
        &self,
        storage: &Self::Storage,
    ) -> impl Future<Output = Result<(), TransactionFileSystemError>> + Send;
}

/// Handle-relative visible mutation portion of the transaction capability.
pub trait TransactionMutation: TransactionStorage {
    fn apply_guarded(
        &self,
        lease: &Self::Lease,
        journal: &Self::Journal,
        mutation: GuardedMutation<'_, Self::ResolvedPath, Self::StagedFile, Self::Backup>,
    ) -> impl Future<Output = Result<MutationOutcome, TransactionFileSystemError>> + Send;

    fn sync_parent(
        &self,
        path: &Self::ResolvedPath,
    ) -> impl Future<Output = Result<(), TransactionFileSystemError>> + Send;

    fn restore_metadata(
        &self,
        path: &Self::ResolvedPath,
        metadata: &MetadataSnapshot,
    ) -> impl Future<Output = Result<(), TransactionFileSystemError>> + Send;
}

/// Restart enumeration and convergence portion of the transaction capability.
pub trait TransactionRecovery: TransactionMutation {
    fn pending_recovery(
        &self,
        root: &Self::Root,
    ) -> impl Future<Output = Result<Vec<DurableTransactionKey>, TransactionFileSystemError>> + Send;

    fn recover(
        &self,
        key: &DurableTransactionKey,
    ) -> impl Future<Output = Result<RecoveryOutcome, TransactionFileSystemError>> + Send;
}

/// Complete executor-owned filesystem contract required before commits are enabled.
///
/// Implementations are expected to keep native handles executor-local and provide
/// all durability and recovery operations. Planner-only callers should depend on
/// [`PlanningFileSystem`] so mutation methods are absent from their dependency.
pub trait TransactionFileSystem: TransactionRecovery {}

impl<T> TransactionFileSystem for T where T: TransactionRecovery {}
