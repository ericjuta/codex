use codex_hashline_transaction::CanonicalPathKey;
use codex_hashline_transaction::DurableFileEvidence;
use codex_hashline_transaction::DurablePathKey;
use codex_hashline_transaction::DurableTransactionKey;
use codex_hashline_transaction::ExecutorRootIdentity;
use codex_hashline_transaction::FileEvidence;
use codex_hashline_transaction::GuardedMutation;
use codex_hashline_transaction::GuardedRollback;
use codex_hashline_transaction::JournalBytes;
use codex_hashline_transaction::LoadedJournal;
use codex_hashline_transaction::MetadataSnapshot;
use codex_hashline_transaction::MutationOutcome;
use codex_hashline_transaction::ObservationLimit;
use codex_hashline_transaction::ObservedEvidence;
use codex_hashline_transaction::ObservedPath;
use codex_hashline_transaction::PlanningFileSystem;
use codex_hashline_transaction::RecoveryScanLimit;
use codex_hashline_transaction::StageFileRequest;
use codex_hashline_transaction::StorageRequirements;
use codex_hashline_transaction::TransactionCoordination;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_hashline_transaction::TransactionId;
use codex_hashline_transaction::TransactionMutation;
use codex_hashline_transaction::TransactionRecovery;
use codex_hashline_transaction::TransactionStorage;
use codex_utils_path_uri::PathUri;

use super::NativePlanningFileSystem;
use super::NativeTransactionFileSystem;

#[derive(Clone, Debug)]
pub struct NativeRoot;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeResolvedPath;

impl PlanningFileSystem for NativePlanningFileSystem {
    type Root = NativeRoot;
    type ResolvedPath = NativeResolvedPath;

    async fn open_root(&self, _root: &PathUri) -> Result<Self::Root, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn resolve(
        &self,
        _root: &Self::Root,
        _model_path: &str,
    ) -> Result<Self::ResolvedPath, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn observe(
        &self,
        _path: &Self::ResolvedPath,
        _limit: ObservationLimit,
    ) -> Result<ObservedPath, TransactionFileSystemError> {
        Err(unsupported())
    }

    fn root_identity(
        &self,
        _root: &Self::Root,
    ) -> Result<ExecutorRootIdentity, TransactionFileSystemError> {
        Err(unsupported())
    }

    fn canonical_path_key(
        &self,
        _path: &Self::ResolvedPath,
    ) -> Result<CanonicalPathKey, TransactionFileSystemError> {
        Err(unsupported())
    }
}

#[derive(Debug)]
pub struct NativeLease;

impl PlanningFileSystem for NativeTransactionFileSystem {
    type Root = NativeRoot;
    type ResolvedPath = NativeResolvedPath;

    async fn open_root(&self, root: &PathUri) -> Result<Self::Root, TransactionFileSystemError> {
        self.ensure_configured_root(root)?;
        Err(unsupported())
    }

    async fn resolve(
        &self,
        _root: &Self::Root,
        _model_path: &str,
    ) -> Result<Self::ResolvedPath, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn observe(
        &self,
        _path: &Self::ResolvedPath,
        _limit: ObservationLimit,
    ) -> Result<ObservedPath, TransactionFileSystemError> {
        Err(unsupported())
    }

    fn root_identity(
        &self,
        _root: &Self::Root,
    ) -> Result<ExecutorRootIdentity, TransactionFileSystemError> {
        Err(unsupported())
    }

    fn canonical_path_key(
        &self,
        _path: &Self::ResolvedPath,
    ) -> Result<CanonicalPathKey, TransactionFileSystemError> {
        Err(unsupported())
    }
}

impl TransactionCoordination for NativeTransactionFileSystem {
    type Lease = NativeLease;

    async fn lock_paths(
        &self,
        _root: &Self::Root,
        _paths: &[Self::ResolvedPath],
    ) -> Result<Self::Lease, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn reobserve_locked(
        &self,
        _lease: &Self::Lease,
        _path: &Self::ResolvedPath,
        _limit: ObservationLimit,
    ) -> Result<ObservedPath, TransactionFileSystemError> {
        Err(unsupported())
    }
}

#[derive(Debug)]
pub struct NativeStorage;

#[derive(Debug)]
pub struct NativeStagedFile;

#[derive(Debug)]
pub struct NativeBackup;

#[derive(Debug)]
pub struct NativeJournal;

impl TransactionStorage for NativeTransactionFileSystem {
    type Storage = NativeStorage;
    type StagedFile = NativeStagedFile;
    type Backup = NativeBackup;
    type Journal = NativeJournal;

    async fn allocate_storage(
        &self,
        _lease: &Self::Lease,
        _transaction_id: &TransactionId,
        _requirements: StorageRequirements,
    ) -> Result<Self::Storage, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn stage_file(
        &self,
        _storage: &Self::Storage,
        _request: StageFileRequest<'_, Self::ResolvedPath>,
    ) -> Result<Self::StagedFile, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn backup_file(
        &self,
        _storage: &Self::Storage,
        _source: &Self::ResolvedPath,
        _expected: &FileEvidence,
    ) -> Result<Self::Backup, TransactionFileSystemError> {
        Err(unsupported())
    }

    fn staged_file_evidence(
        &self,
        _staged: &Self::StagedFile,
    ) -> Result<DurableFileEvidence, TransactionFileSystemError> {
        Err(unsupported())
    }

    fn backup_evidence(
        &self,
        _backup: &Self::Backup,
    ) -> Result<DurableFileEvidence, TransactionFileSystemError> {
        Err(unsupported())
    }

    fn durable_root_key(
        &self,
        _root: &Self::Root,
    ) -> Result<DurablePathKey, TransactionFileSystemError> {
        Err(unsupported())
    }

    fn durable_path_key(
        &self,
        _path: &Self::ResolvedPath,
    ) -> Result<DurablePathKey, TransactionFileSystemError> {
        Err(unsupported())
    }

    fn durable_transaction_key(
        &self,
        _storage: &Self::Storage,
    ) -> Result<DurableTransactionKey, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn persist_journal(
        &self,
        _storage: &Self::Storage,
        _journal: &JournalBytes,
    ) -> Result<Self::Journal, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn sync_staged_file(
        &self,
        _staged: &Self::StagedFile,
    ) -> Result<(), TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn sync_backup(&self, _backup: &Self::Backup) -> Result<(), TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn sync_journal(
        &self,
        _journal: &Self::Journal,
    ) -> Result<(), TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn sync_storage(
        &self,
        _storage: &Self::Storage,
    ) -> Result<(), TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn cleanup_artifacts(
        &self,
        _storage: &Self::Storage,
    ) -> Result<(), TransactionFileSystemError> {
        Err(unsupported())
    }
}

impl TransactionMutation for NativeTransactionFileSystem {
    async fn apply_guarded(
        &self,
        _lease: &Self::Lease,
        _journal: &Self::Journal,
        _mutation: GuardedMutation<'_, Self::ResolvedPath, Self::StagedFile, Self::Backup>,
    ) -> Result<MutationOutcome, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn restore_guarded(
        &self,
        _lease: &Self::Lease,
        _journal: &Self::Journal,
        _rollback: GuardedRollback<'_, Self::ResolvedPath, Self::StagedFile, Self::Backup>,
    ) -> Result<MutationOutcome, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn sync_parent(
        &self,
        _path: &Self::ResolvedPath,
    ) -> Result<(), TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn restore_metadata(
        &self,
        _path: &Self::ResolvedPath,
        _metadata: &MetadataSnapshot,
    ) -> Result<(), TransactionFileSystemError> {
        Err(unsupported())
    }
}

impl TransactionRecovery for NativeTransactionFileSystem {
    fn recovery_environment_id(&self) -> Result<String, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn pending_recovery(
        &self,
        _limit: RecoveryScanLimit,
    ) -> Result<Vec<DurableTransactionKey>, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn lock_recovery_storage(
        &self,
        _key: &DurableTransactionKey,
    ) -> Result<Self::Storage, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn load_journal(
        &self,
        _storage: &Self::Storage,
        _max_bytes: u64,
    ) -> Result<LoadedJournal<Self::Journal>, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn reopen_root(
        &self,
        _key: &DurablePathKey,
    ) -> Result<Self::Root, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn reopen_path(
        &self,
        _root: &Self::Root,
        _key: &DurablePathKey,
    ) -> Result<Self::ResolvedPath, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn reopen_staged_file(
        &self,
        _storage: &Self::Storage,
        _evidence: &DurableFileEvidence,
    ) -> Result<Self::StagedFile, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn reopen_backup(
        &self,
        _storage: &Self::Storage,
        _evidence: &DurableFileEvidence,
    ) -> Result<Self::Backup, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn observe_evidence_locked(
        &self,
        _lease: &Self::Lease,
        _path: &Self::ResolvedPath,
        _limit: ObservationLimit,
    ) -> Result<ObservedEvidence, TransactionFileSystemError> {
        Err(unsupported())
    }
}

fn unsupported() -> TransactionFileSystemError {
    TransactionFileSystemError::Unsupported {
        capability: "hashline transaction planning",
        reason: "native no-follow path handles are not implemented on this executor platform"
            .to_string(),
    }
}
