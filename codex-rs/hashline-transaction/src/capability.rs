use std::fmt::Debug;
use std::future::Future;

use codex_utils_path_uri::PathUri;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

use crate::MetadataSnapshot;
use crate::ObservationLimit;
use crate::ObservedPath;
use crate::journal::DurableFileEvidence;
use crate::journal::FileEvidence;
use crate::journal::JournalBytes;
use crate::journal::StorageRequirements;

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
    #[error("transaction path `{path}` changed since planning")]
    ChangedSincePlanning { path: String },
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
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableTransactionKey {
    pub namespace: String,
    pub value: Vec<u8>,
}

/// Hard cap for one environment-owned recovery scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryScanLimit {
    pub max_transactions: u64,
}

/// Collision-resistant transaction identifier chosen by the transaction engine.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransactionId(pub String);

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
        expected: &'a FileEvidence,
        staged: &'a S,
        backup: &'a B,
    },
    Remove {
        source: &'a P,
        expected: &'a FileEvidence,
        backup: &'a B,
    },
    Move {
        source: &'a P,
        expected: &'a FileEvidence,
        destination: &'a P,
        staged: Option<&'a S>,
        backup: &'a B,
    },
}

/// One inverse mutation that must preserve any externally disturbed path.
#[derive(Debug)]
pub enum GuardedRollback<'a, P, S, B> {
    RemoveCreated {
        destination: &'a P,
        staged: &'a S,
    },
    RestoreReplaced {
        destination: &'a P,
        expected: &'a FileEvidence,
        staged: &'a S,
        backup: &'a B,
    },
    RestoreRemoved {
        source: &'a P,
        expected: &'a FileEvidence,
        backup: &'a B,
    },
    RestoreMove {
        source: &'a P,
        expected: &'a FileEvidence,
        destination: &'a P,
        staged: &'a S,
        backup: &'a B,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationOutcome {
    Applied,
    AlreadyApplied,
}

/// Compact live evidence used by restart recovery without retaining file contents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservedEvidence {
    Absent,
    Present(FileEvidence),
}

/// Current durable journal handle and its bounded serialized record.
#[derive(Debug)]
pub struct LoadedJournal<J> {
    pub journal: J,
    pub bytes: JournalBytes,
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

    /// Exclusively creates storage for a new collision-resistant transaction ID.
    ///
    /// An existing ID must fail without waiting for recovery ownership; allocation must never
    /// acquire the existing-transaction lease used by [`TransactionRecovery::lock_recovery_storage`].
    fn allocate_storage(
        &self,
        lease: &Self::Lease,
        transaction_id: &TransactionId,
        requirements: StorageRequirements,
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
        expected: &FileEvidence,
    ) -> impl Future<Output = Result<Self::Backup, TransactionFileSystemError>> + Send;

    fn staged_file_evidence(
        &self,
        staged: &Self::StagedFile,
    ) -> Result<DurableFileEvidence, TransactionFileSystemError>;

    fn backup_evidence(
        &self,
        backup: &Self::Backup,
    ) -> Result<DurableFileEvidence, TransactionFileSystemError>;

    fn durable_root_key(
        &self,
        root: &Self::Root,
    ) -> Result<DurablePathKey, TransactionFileSystemError>;

    fn durable_path_key(
        &self,
        path: &Self::ResolvedPath,
    ) -> Result<DurablePathKey, TransactionFileSystemError>;

    fn durable_transaction_key(
        &self,
        storage: &Self::Storage,
    ) -> Result<DurableTransactionKey, TransactionFileSystemError>;

    /// Atomically installs one complete, parseable journal record.
    ///
    /// Replacing an existing record must leave either the previous record or the supplied
    /// record available after failure; implementations must never expose a partial record.
    fn persist_journal(
        &self,
        storage: &Self::Storage,
        journal: &JournalBytes,
    ) -> impl Future<Output = Result<Self::Journal, TransactionFileSystemError>> + Send;

    fn sync_staged_file(
        &self,
        staged: &Self::StagedFile,
    ) -> impl Future<Output = Result<(), TransactionFileSystemError>> + Send;

    fn sync_backup(
        &self,
        backup: &Self::Backup,
    ) -> impl Future<Output = Result<(), TransactionFileSystemError>> + Send;

    fn sync_journal(
        &self,
        journal: &Self::Journal,
    ) -> impl Future<Output = Result<(), TransactionFileSystemError>> + Send;

    fn sync_storage(
        &self,
        storage: &Self::Storage,
    ) -> impl Future<Output = Result<(), TransactionFileSystemError>> + Send;

    /// Durably removes staged files and backups while preserving the journal record.
    ///
    /// A successful call makes the removal restart-stable. Incomplete journal records and
    /// recovery evidence must not be removed by this operation.
    fn cleanup_artifacts(
        &self,
        storage: &Self::Storage,
    ) -> impl Future<Output = Result<(), TransactionFileSystemError>> + Send;
}

/// Handle-relative visible mutation portion of the transaction capability.
pub trait TransactionMutation: TransactionStorage {
    /// Applies one mutation only when the live paths still match its guarded evidence.
    ///
    /// Implementations must compare identity, contents, metadata, and link topology through
    /// retained no-follow handles. A retry may return [`MutationOutcome::AlreadyApplied`] only
    /// when the visible state is proven to be the exact staged result; unknown state fails.
    fn apply_guarded(
        &self,
        lease: &Self::Lease,
        journal: &Self::Journal,
        mutation: GuardedMutation<'_, Self::ResolvedPath, Self::StagedFile, Self::Backup>,
    ) -> impl Future<Output = Result<MutationOutcome, TransactionFileSystemError>> + Send;

    /// Restores one mutation only when the live paths match a legal before, after, or restored state.
    ///
    /// Implementations must return [`MutationOutcome::AlreadyApplied`] when the exact planned
    /// before-image is still visible (and, for a move, its destination is absent), when a create
    /// destination is already absent, or when the exact backup result is already visible. A staged
    /// after-image may be replaced or removed to perform the rollback. Every comparison includes
    /// identity, contents, metadata, and link topology; unknown or externally disturbed state must
    /// be preserved and reported as an error.
    fn restore_guarded(
        &self,
        lease: &Self::Lease,
        journal: &Self::Journal,
        rollback: GuardedRollback<'_, Self::ResolvedPath, Self::StagedFile, Self::Backup>,
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
    /// Returns the stable environment identifier that durable journals must match.
    fn recovery_environment_id(&self) -> Result<String, TransactionFileSystemError>;

    /// Enumerates bounded durable transaction keys that are not known to be complete.
    fn pending_recovery(
        &self,
        limit: RecoveryScanLimit,
    ) -> impl Future<Output = Result<Vec<DurableTransactionKey>, TransactionFileSystemError>> + Send;

    /// Acquires exclusive recovery ownership for one durable transaction.
    ///
    /// The returned storage handle must retain that ownership for its lifetime. Concurrent calls
    /// for the same key must serialize before either caller can load the journal.
    fn lock_recovery_storage(
        &self,
        key: &DurableTransactionKey,
    ) -> impl Future<Output = Result<Self::Storage, TransactionFileSystemError>> + Send;

    /// Loads the latest durable journal generation while recovery ownership is held.
    fn load_journal(
        &self,
        storage: &Self::Storage,
        max_bytes: u64,
    ) -> impl Future<Output = Result<LoadedJournal<Self::Journal>, TransactionFileSystemError>> + Send;

    fn reopen_root(
        &self,
        key: &DurablePathKey,
    ) -> impl Future<Output = Result<Self::Root, TransactionFileSystemError>> + Send;

    fn reopen_path(
        &self,
        root: &Self::Root,
        key: &DurablePathKey,
    ) -> impl Future<Output = Result<Self::ResolvedPath, TransactionFileSystemError>> + Send;

    fn reopen_staged_file(
        &self,
        storage: &Self::Storage,
        evidence: &DurableFileEvidence,
    ) -> impl Future<Output = Result<Self::StagedFile, TransactionFileSystemError>> + Send;

    fn reopen_backup(
        &self,
        storage: &Self::Storage,
        evidence: &DurableFileEvidence,
    ) -> impl Future<Output = Result<Self::Backup, TransactionFileSystemError>> + Send;

    fn observe_evidence_locked(
        &self,
        lease: &Self::Lease,
        path: &Self::ResolvedPath,
        limit: ObservationLimit,
    ) -> impl Future<Output = Result<ObservedEvidence, TransactionFileSystemError>> + Send;
}

/// Complete executor-owned filesystem contract required before commits are enabled.
///
/// Implementations are expected to keep native handles executor-local and provide
/// all durability and recovery operations. Planner-only callers should depend on
/// [`PlanningFileSystem`] so mutation methods are absent from their dependency.
pub trait TransactionFileSystem: TransactionRecovery {}

impl<T> TransactionFileSystem for T where T: TransactionRecovery {}
