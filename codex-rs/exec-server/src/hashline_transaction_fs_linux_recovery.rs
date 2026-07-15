use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::sync::Arc;

use codex_hashline_transaction::DurableFileEvidence;
use codex_hashline_transaction::DurablePathKey;
use codex_hashline_transaction::DurableTransactionKey;
use codex_hashline_transaction::LoadedJournal;
use codex_hashline_transaction::ObservationLimit;
use codex_hashline_transaction::ObservedEvidence;
use codex_hashline_transaction::PlanningFileSystem;
use codex_hashline_transaction::RecoveryScanLimit;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_hashline_transaction::TransactionRecovery;

use super::observe;
use super::open_root;
use super::recovery_io::ArtifactClass;
use super::recovery_io::reopen_artifact;
use super::recovery_io::reopen_path_from_key;
use super::recovery_io::transaction_id_from_key;
use super::recovery_scan::load_journal_from_directory;
use super::recovery_scan::pending_recovery_keys;
use super::run_blocking;
use super::storage::NativeBackup;
use super::storage::NativeStagedFile;
use super::storage::reopen_storage;
use super::storage_io::platform_error;
use crate::hashline_transaction_fs::NativeTransactionFileSystem;

const ROOT_KEY_NAMESPACE: &str = "linux-hashline-root-v1";

impl TransactionRecovery for NativeTransactionFileSystem {
    fn recovery_environment_id(&self) -> Result<String, TransactionFileSystemError> {
        Ok(self.environment_id().to_string())
    }

    async fn pending_recovery(
        &self,
        limit: RecoveryScanLimit,
    ) -> Result<Vec<DurableTransactionKey>, TransactionFileSystemError> {
        let root = self.open_root(&self.root).await?;
        let environment_id = self.environment_id().to_string();
        run_blocking("scan transaction recovery storage", move || {
            pending_recovery_keys(&root, &environment_id, limit)
        })
        .await
    }

    async fn lock_recovery_storage(
        &self,
        key: &DurableTransactionKey,
    ) -> Result<Self::Storage, TransactionFileSystemError> {
        let root = self.open_root(&self.root).await?;
        let transaction_id = transaction_id_from_key(&root, key)?;
        run_blocking("lock recovery storage", move || {
            let storage = reopen_storage(root, transaction_id)?;
            lock_file(&storage._reservation)?;
            Ok(storage)
        })
        .await
    }

    async fn load_journal(
        &self,
        storage: &Self::Storage,
        max_bytes: u64,
    ) -> Result<LoadedJournal<Self::Journal>, TransactionFileSystemError> {
        let directory = Arc::clone(&storage.directory);
        run_blocking("load transaction journal", move || {
            load_journal_from_directory(directory, max_bytes)
        })
        .await
    }

    async fn reopen_root(
        &self,
        key: &DurablePathKey,
    ) -> Result<Self::Root, TransactionFileSystemError> {
        if key.namespace != ROOT_KEY_NAMESPACE {
            return Err(TransactionFileSystemError::Unsupported {
                capability: "native transaction recovery root",
                reason: "durable root key namespace is not supported".to_string(),
            });
        }
        let root = open_root(&self.root)?;
        if root.identity.value != key.value {
            return Err(TransactionFileSystemError::ChangedSincePlanning {
                path: self.root.to_string(),
            });
        }
        Ok(root)
    }

    async fn reopen_path(
        &self,
        root: &Self::Root,
        key: &DurablePathKey,
    ) -> Result<Self::ResolvedPath, TransactionFileSystemError> {
        let root = root.clone();
        let key = key.clone();
        run_blocking("reopen transaction recovery path", move || {
            reopen_path_from_key(&root, &key)
        })
        .await
    }

    async fn reopen_staged_file(
        &self,
        storage: &Self::Storage,
        evidence: &DurableFileEvidence,
    ) -> Result<Self::StagedFile, TransactionFileSystemError> {
        let storage = storage.clone();
        let evidence = evidence.clone();
        run_blocking("reopen staged transaction artifact", move || {
            let artifact = reopen_artifact(&storage, &evidence, ArtifactClass::Staged)?;
            Ok(NativeStagedFile {
                parent: artifact.parent,
                name: artifact.name,
                file: artifact.file,
                evidence,
            })
        })
        .await
    }

    async fn reopen_backup(
        &self,
        storage: &Self::Storage,
        evidence: &DurableFileEvidence,
    ) -> Result<Self::Backup, TransactionFileSystemError> {
        let storage = storage.clone();
        let evidence = evidence.clone();
        run_blocking("reopen transaction backup", move || {
            let artifact = reopen_artifact(&storage, &evidence, ArtifactClass::Backup)?;
            Ok(NativeBackup {
                parent: artifact.parent,
                name: artifact.name,
                file: artifact.file,
                evidence,
            })
        })
        .await
    }

    async fn observe_evidence_locked(
        &self,
        lease: &Self::Lease,
        path: &Self::ResolvedPath,
        limit: ObservationLimit,
    ) -> Result<ObservedEvidence, TransactionFileSystemError> {
        lease.require_path(path, "observe recovery evidence")?;
        let path = path.clone();
        let observed =
            run_blocking("observe recovery evidence", move || observe(&path, limit)).await?;
        Ok(match observed {
            codex_hashline_transaction::ObservedPath::Absent => ObservedEvidence::Absent,
            codex_hashline_transaction::ObservedPath::Present(observed) => {
                ObservedEvidence::Present((&observed).into())
            }
        })
    }
}

fn lock_file(file: &File) -> Result<(), TransactionFileSystemError> {
    loop {
        // SAFETY: the reservation descriptor remains owned by `NativeStorage` for the lock lifetime.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(platform_error("lock transaction recovery storage", error));
        }
    }
}
