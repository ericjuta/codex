use crate::DurableFileEvidence;
use crate::DurablePathKey;
use crate::DurableTransactionKey;
use crate::FileEvidence;
use crate::LoadedJournal;
use crate::ObservationLimit;
use crate::ObservedEvidence;
use crate::ObservedPath;
use crate::RecoveryScanLimit;
use crate::TransactionFileSystemError;
use crate::TransactionId;
use crate::TransactionRecovery;

use super::executor_test_support::TestFileSystem;
use super::executor_test_support::TestJournal;
use super::executor_test_support::TestStorage;
use super::executor_test_support::changed;
use super::executor_test_support::durable_evidence;
use super::executor_test_support::observe;
use super::executor_test_support::platform;

impl TransactionRecovery for TestFileSystem {
    async fn pending_recovery(
        &self,
        _limit: RecoveryScanLimit,
    ) -> Result<Vec<DurableTransactionKey>, TransactionFileSystemError> {
        Ok(Vec::new())
    }

    async fn open_recovery_storage(
        &self,
        key: &DurableTransactionKey,
    ) -> Result<Self::Storage, TransactionFileSystemError> {
        let transaction_id = decode_transaction_key(key)?;
        Ok(TestStorage {
            transaction_id: TransactionId(transaction_id),
        })
    }

    async fn load_journal(
        &self,
        storage: &Self::Storage,
        max_bytes: u64,
    ) -> Result<LoadedJournal<Self::Journal>, TransactionFileSystemError> {
        let state = self.state.lock().unwrap();
        let (generation, record) = state
            .journals
            .iter()
            .enumerate()
            .rev()
            .find(|(_, record)| record.transaction_id == storage.transaction_id)
            .ok_or_else(|| platform("load journal", "missing journal"))?;
        let bytes = record
            .to_bounded_json(max_bytes)
            .map_err(|error| platform("load journal", error.to_string()))?;
        Ok(LoadedJournal {
            journal: TestJournal {
                transaction_id: storage.transaction_id.clone(),
                generation: generation + 1,
            },
            bytes,
        })
    }

    async fn reopen_root(
        &self,
        key: &DurablePathKey,
    ) -> Result<Self::Root, TransactionFileSystemError> {
        decode_key("test-root", key)
    }

    async fn reopen_path(
        &self,
        _root: &Self::Root,
        key: &DurablePathKey,
    ) -> Result<Self::ResolvedPath, TransactionFileSystemError> {
        decode_key("test-path", key)
    }

    async fn reopen_staged_file(
        &self,
        _storage: &Self::Storage,
        evidence: &DurableFileEvidence,
    ) -> Result<Self::StagedFile, TransactionFileSystemError> {
        let key = decode_key("test-artifact", &evidence.key)?;
        let staged = self
            .state
            .lock()
            .unwrap()
            .staged
            .get(&key)
            .cloned()
            .ok_or_else(|| platform("reopen staged", key.clone()))?;
        if durable_evidence(&key, &staged.file) != *evidence {
            return Err(changed(&key));
        }
        Ok(staged)
    }

    async fn reopen_backup(
        &self,
        _storage: &Self::Storage,
        evidence: &DurableFileEvidence,
    ) -> Result<Self::Backup, TransactionFileSystemError> {
        let key = decode_key("test-artifact", &evidence.key)?;
        let backup = self
            .state
            .lock()
            .unwrap()
            .backups
            .get(&key)
            .cloned()
            .ok_or_else(|| platform("reopen backup", key.clone()))?;
        if durable_evidence(&key, &backup.file) != *evidence {
            return Err(changed(&key));
        }
        Ok(backup)
    }

    async fn observe_evidence_locked(
        &self,
        _lease: &Self::Lease,
        path: &Self::ResolvedPath,
        _limit: ObservationLimit,
    ) -> Result<ObservedEvidence, TransactionFileSystemError> {
        Ok(match observe(&self.state.lock().unwrap(), path) {
            ObservedPath::Absent => ObservedEvidence::Absent,
            ObservedPath::Present(file) => ObservedEvidence::Present(FileEvidence::from(&file)),
        })
    }
}

fn decode_key(
    namespace: &'static str,
    key: &DurablePathKey,
) -> Result<String, TransactionFileSystemError> {
    if key.namespace != namespace {
        return Err(platform("decode durable key", "unexpected namespace"));
    }
    String::from_utf8(key.value.clone())
        .map_err(|error| platform("decode durable key", error.to_string()))
}

fn decode_transaction_key(
    key: &DurableTransactionKey,
) -> Result<String, TransactionFileSystemError> {
    if key.namespace != "test-transaction" {
        return Err(platform(
            "decode durable transaction key",
            "unexpected namespace",
        ));
    }
    String::from_utf8(key.value.clone())
        .map_err(|error| platform("decode durable transaction key", error.to_string()))
}
