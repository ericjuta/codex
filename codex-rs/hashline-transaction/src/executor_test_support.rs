use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::time::Duration;

use codex_utils_path_uri::PathUri;

use crate::CanonicalPathKey;
use crate::DurableFileEvidence;
use crate::DurablePathKey;
use crate::DurableTransactionKey;
use crate::ExecutorFileIdentity;
use crate::ExecutorRootIdentity;
use crate::FileEvidence;
use crate::FileKind;
use crate::GuardedMutation;
use crate::GuardedRollback;
use crate::JournalBytes;
use crate::JournalRecord;
use crate::MetadataSnapshot;
use crate::MutationOutcome;
use crate::ObservationLimit;
use crate::ObservedFile;
use crate::ObservedPath;
use crate::PlanningFileSystem;
use crate::StageFileRequest;
use crate::StorageRequirements;
use crate::TransactionCoordination;
use crate::TransactionFileSystemError;
use crate::TransactionId;
use crate::TransactionMutation;
use crate::TransactionStorage;

use super::executor_test_mutation::apply;
use super::executor_test_mutation::require_file;
use super::executor_test_mutation::restore;

#[derive(Clone, Debug)]
pub(super) struct TestFileSystem {
    pub(super) state: Arc<Mutex<TestState>>,
    pub(super) recovery_locks: Arc<Mutex<BTreeMap<String, Arc<futures::lock::Mutex<()>>>>>,
    path_locks: Arc<Mutex<BTreeMap<String, Arc<futures::lock::Mutex<()>>>>>,
    path_lock_attempts: Arc<(Mutex<usize>, Condvar)>,
    persist_pause: Arc<(Mutex<PersistPause>, Condvar)>,
}

#[derive(Debug, Default)]
pub(super) struct TestState {
    pub(super) files: BTreeMap<String, ObservedFile>,
    pub(super) staged: BTreeMap<String, TestStagedFile>,
    pub(super) backups: BTreeMap<String, TestBackup>,
    pub(super) journals: Vec<JournalRecord>,
    pub(super) allocated_transactions: BTreeSet<String>,
    pub(super) pending_recovery_override: Option<Vec<DurableTransactionKey>>,
    events: Vec<TestEvent>,
    next_artifact: usize,
    persist_calls: usize,
    apply_calls: usize,
    restore_calls: usize,
    fail_persist_at: Option<usize>,
    fail_apply_at: Option<usize>,
    fail_restore_at: Option<usize>,
    crash_after_persist_at: Option<usize>,
    crash_after_apply_at: Option<usize>,
    crash_after_restore_at: Option<usize>,
    crash_after_cleanup: bool,
    pub(super) journal_load_calls: usize,
}

#[derive(Debug, Default)]
struct PersistPause {
    persist_call: Option<usize>,
    reached: bool,
    released: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum TestEvent {
    Locked(Vec<String>),
    Applied(String),
    Restored(String),
    Cleaned,
}

#[derive(Debug)]
pub(super) struct TestLease {
    _guards: Vec<futures::lock::OwnedMutexGuard<()>>,
}

#[derive(Debug)]
pub(super) struct TestStorage {
    pub(super) transaction_id: TransactionId,
    pub(super) _recovery_lease: Option<futures::lock::OwnedMutexGuard<()>>,
}

#[derive(Clone, Debug)]
pub(super) struct TestStagedFile {
    key: String,
    pub(super) transaction_id: TransactionId,
    pub(super) destination: String,
    pub(super) file: ObservedFile,
}

#[derive(Clone, Debug)]
pub(super) struct TestBackup {
    key: String,
    pub(super) transaction_id: TransactionId,
    pub(super) source: String,
    pub(super) before: FileEvidence,
    pub(super) file: ObservedFile,
}

#[derive(Clone, Debug)]
pub(super) struct TestJournal {
    pub(super) transaction_id: TransactionId,
    pub(super) generation: usize,
}

impl TestFileSystem {
    pub(super) fn new(files: impl IntoIterator<Item = (&'static str, &'static [u8], u8)>) -> Self {
        let files = files
            .into_iter()
            .map(|(path, contents, identity)| (path.to_string(), observed_file(contents, identity)))
            .collect();
        Self {
            state: Arc::new(Mutex::new(TestState {
                files,
                ..TestState::default()
            })),
            recovery_locks: Arc::new(Mutex::new(BTreeMap::new())),
            path_locks: Arc::new(Mutex::new(BTreeMap::new())),
            path_lock_attempts: Arc::new((Mutex::new(0), Condvar::new())),
            persist_pause: Arc::new((Mutex::new(PersistPause::default()), Condvar::new())),
        }
    }

    pub(super) fn files(&self) -> BTreeMap<String, Vec<u8>> {
        self.state
            .lock()
            .unwrap()
            .files
            .iter()
            .map(|(path, file)| (path.clone(), file.contents.clone()))
            .collect()
    }

    pub(super) fn journals(&self) -> Vec<JournalRecord> {
        self.state.lock().unwrap().journals.clone()
    }

    pub(super) fn restart(&self) -> Self {
        self.clone()
    }

    pub(super) fn transaction_key(transaction_id: &str) -> DurableTransactionKey {
        DurableTransactionKey {
            namespace: "test-transaction".to_string(),
            value: transaction_id.as_bytes().to_vec(),
        }
    }

    pub(super) fn events(&self) -> Vec<TestEvent> {
        self.state.lock().unwrap().events.clone()
    }

    pub(super) fn artifact_counts(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (state.staged.len(), state.backups.len())
    }

    pub(super) fn external_write(&self, path: &str, contents: &[u8], identity: u8) {
        self.state
            .lock()
            .unwrap()
            .files
            .insert(path.to_string(), observed_file(contents, identity));
    }

    pub(super) fn fail_persist_at(&self, call: usize) {
        self.state.lock().unwrap().fail_persist_at = Some(call);
    }

    pub(super) fn fail_apply_at(&self, call: usize) {
        self.state.lock().unwrap().fail_apply_at = Some(call);
    }

    pub(super) fn fail_restore_at(&self, call: usize) {
        self.state.lock().unwrap().fail_restore_at = Some(call);
    }

    pub(super) fn crash_after_persist_at(&self, call: usize) {
        self.state.lock().unwrap().crash_after_persist_at = Some(call);
    }

    pub(super) fn crash_after_apply_at(&self, call: usize) {
        self.state.lock().unwrap().crash_after_apply_at = Some(call);
    }

    pub(super) fn crash_after_restore_at(&self, call: usize) {
        self.state.lock().unwrap().crash_after_restore_at = Some(call);
    }

    pub(super) fn crash_after_cleanup(&self) {
        self.state.lock().unwrap().crash_after_cleanup = true;
    }

    pub(super) fn set_pending_recovery(&self, keys: Vec<DurableTransactionKey>) {
        self.state.lock().unwrap().pending_recovery_override = Some(keys);
    }

    pub(super) fn pause_after_persist_at(&self, call: usize) {
        let mut pause = self.persist_pause.0.lock().unwrap();
        *pause = PersistPause {
            persist_call: Some(call),
            ..PersistPause::default()
        };
    }

    pub(super) fn wait_for_persist_pause(&self) {
        let pause = self.persist_pause.0.lock().unwrap();
        let (_pause, wait) = self
            .persist_pause
            .1
            .wait_timeout_while(pause, Duration::from_secs(5), |pause| !pause.reached)
            .unwrap();
        assert!(
            !wait.timed_out(),
            "timed out waiting for journal persistence pause"
        );
    }

    pub(super) fn resume_persist(&self) {
        let mut pause = self.persist_pause.0.lock().unwrap();
        pause.released = true;
        self.persist_pause.1.notify_all();
    }

    pub(super) fn wait_for_path_lock_attempts(&self, minimum: usize) {
        let attempts = self.path_lock_attempts.0.lock().unwrap();
        let (_attempts, wait) = self
            .path_lock_attempts
            .1
            .wait_timeout_while(attempts, Duration::from_secs(5), |attempts| {
                *attempts < minimum
            })
            .unwrap();
        assert!(!wait.timed_out(), "timed out waiting for path lock attempt");
    }

    pub(super) fn journal_load_calls(&self) -> usize {
        self.state.lock().unwrap().journal_load_calls
    }
}

pub(super) fn observed_file(contents: &[u8], identity: u8) -> ObservedFile {
    ObservedFile::new(
        contents.to_vec(),
        ExecutorFileIdentity {
            namespace: "test-file".to_string(),
            value: vec![identity],
        },
        MetadataSnapshot::new("test-metadata".to_string(), b"mode=0644".to_vec()),
        NonZeroU64::MIN,
        FileKind::File,
    )
}

impl PlanningFileSystem for TestFileSystem {
    type Root = String;
    type ResolvedPath = String;

    async fn open_root(&self, root: &PathUri) -> Result<Self::Root, TransactionFileSystemError> {
        Ok(root.to_string())
    }

    async fn resolve(
        &self,
        _root: &Self::Root,
        model_path: &str,
    ) -> Result<Self::ResolvedPath, TransactionFileSystemError> {
        Ok(model_path.to_string())
    }

    async fn observe(
        &self,
        path: &Self::ResolvedPath,
        _limit: ObservationLimit,
    ) -> Result<ObservedPath, TransactionFileSystemError> {
        Ok(observe(&self.state.lock().unwrap(), path))
    }

    fn root_identity(
        &self,
        root: &Self::Root,
    ) -> Result<ExecutorRootIdentity, TransactionFileSystemError> {
        Ok(ExecutorRootIdentity {
            namespace: "test-root".to_string(),
            value: root.as_bytes().to_vec(),
        })
    }

    fn canonical_path_key(
        &self,
        path: &Self::ResolvedPath,
    ) -> Result<CanonicalPathKey, TransactionFileSystemError> {
        Ok(CanonicalPathKey {
            namespace: "test-path".to_string(),
            value: path.as_bytes().to_vec(),
        })
    }
}

impl TransactionCoordination for TestFileSystem {
    type Lease = TestLease;

    async fn lock_paths(
        &self,
        root: &Self::Root,
        paths: &[Self::ResolvedPath],
    ) -> Result<Self::Lease, TransactionFileSystemError> {
        {
            let mut attempts = self.path_lock_attempts.0.lock().unwrap();
            *attempts += 1;
            self.path_lock_attempts.1.notify_all();
        }
        let locks = {
            let mut path_locks = self.path_locks.lock().unwrap();
            paths
                .iter()
                .map(|path| {
                    path_locks
                        .entry(format!("{root}\0{path}"))
                        .or_insert_with(|| Arc::new(futures::lock::Mutex::new(())))
                        .clone()
                })
                .collect::<Vec<_>>()
        };
        let mut guards = Vec::with_capacity(locks.len());
        for lock in locks {
            guards.push(lock.lock_owned().await);
        }
        self.state
            .lock()
            .unwrap()
            .events
            .push(TestEvent::Locked(paths.to_vec()));
        Ok(TestLease { _guards: guards })
    }

    async fn reobserve_locked(
        &self,
        _lease: &Self::Lease,
        path: &Self::ResolvedPath,
        _limit: ObservationLimit,
    ) -> Result<ObservedPath, TransactionFileSystemError> {
        Ok(observe(&self.state.lock().unwrap(), path))
    }
}

impl TransactionStorage for TestFileSystem {
    type Storage = TestStorage;
    type StagedFile = TestStagedFile;
    type Backup = TestBackup;
    type Journal = TestJournal;

    async fn allocate_storage(
        &self,
        _lease: &Self::Lease,
        transaction_id: &TransactionId,
        _requirements: StorageRequirements,
    ) -> Result<Self::Storage, TransactionFileSystemError> {
        let mut state = self.state.lock().unwrap();
        if !state
            .allocated_transactions
            .insert(transaction_id.0.clone())
        {
            return Err(platform(
                "allocate storage",
                "transaction ID already exists",
            ));
        }
        Ok(TestStorage {
            transaction_id: transaction_id.clone(),
            _recovery_lease: None,
        })
    }

    async fn stage_file(
        &self,
        storage: &Self::Storage,
        request: StageFileRequest<'_, Self::ResolvedPath>,
    ) -> Result<Self::StagedFile, TransactionFileSystemError> {
        let mut state = self.state.lock().unwrap();
        let index = state.next_artifact;
        state.next_artifact += 1;
        let key = format!("staged-{index}");
        let metadata = request.metadata.cloned().unwrap_or_else(|| {
            MetadataSnapshot::new("test-metadata".to_string(), b"mode=0644".to_vec())
        });
        let staged = TestStagedFile {
            key: key.clone(),
            transaction_id: storage.transaction_id.clone(),
            destination: request.destination.clone(),
            file: ObservedFile::new(
                request.contents.to_vec(),
                ExecutorFileIdentity {
                    namespace: "test-staged".to_string(),
                    value: index.to_be_bytes().to_vec(),
                },
                metadata,
                NonZeroU64::MIN,
                FileKind::File,
            ),
        };
        state.staged.insert(key, staged.clone());
        Ok(staged)
    }

    async fn backup_file(
        &self,
        storage: &Self::Storage,
        source: &Self::ResolvedPath,
        expected: &FileEvidence,
    ) -> Result<Self::Backup, TransactionFileSystemError> {
        let mut state = self.state.lock().unwrap();
        require_file(&state, source, expected, "backup file")?;
        let index = state.next_artifact;
        state.next_artifact += 1;
        let mut file = state.files[source].clone();
        file.identity = ExecutorFileIdentity {
            namespace: "test-backup".to_string(),
            value: index.to_be_bytes().to_vec(),
        };
        let key = format!("backup-{index}");
        let backup = TestBackup {
            key: key.clone(),
            transaction_id: storage.transaction_id.clone(),
            source: source.clone(),
            before: expected.clone(),
            file,
        };
        state.backups.insert(key, backup.clone());
        Ok(backup)
    }

    fn staged_file_evidence(
        &self,
        staged: &Self::StagedFile,
    ) -> Result<DurableFileEvidence, TransactionFileSystemError> {
        Ok(durable_evidence(&staged.key, &staged.file))
    }

    fn backup_evidence(
        &self,
        backup: &Self::Backup,
    ) -> Result<DurableFileEvidence, TransactionFileSystemError> {
        Ok(durable_evidence(&backup.key, &backup.file))
    }

    fn durable_root_key(
        &self,
        root: &Self::Root,
    ) -> Result<DurablePathKey, TransactionFileSystemError> {
        Ok(durable_key("test-root", root))
    }

    fn durable_path_key(
        &self,
        path: &Self::ResolvedPath,
    ) -> Result<DurablePathKey, TransactionFileSystemError> {
        Ok(durable_key("test-path", path))
    }

    fn durable_transaction_key(
        &self,
        storage: &Self::Storage,
    ) -> Result<DurableTransactionKey, TransactionFileSystemError> {
        Ok(DurableTransactionKey {
            namespace: "test-transaction".to_string(),
            value: storage.transaction_id.0.as_bytes().to_vec(),
        })
    }

    async fn persist_journal(
        &self,
        storage: &Self::Storage,
        journal: &JournalBytes,
    ) -> Result<Self::Journal, TransactionFileSystemError> {
        let record: JournalRecord = serde_json::from_slice(journal.as_bytes())
            .map_err(|error| platform("persist journal", error.to_string()))?;
        let mut state = self.state.lock().unwrap();
        state.persist_calls += 1;
        if state.fail_persist_at == Some(state.persist_calls) {
            state.fail_persist_at = None;
            return Err(platform("persist journal", "injected failure"));
        }
        state.journals.push(record);
        Ok(TestJournal {
            transaction_id: storage.transaction_id.clone(),
            generation: state.journals.len(),
        })
    }

    async fn sync_staged_file(
        &self,
        _staged: &Self::StagedFile,
    ) -> Result<(), TransactionFileSystemError> {
        Ok(())
    }

    async fn sync_backup(&self, _backup: &Self::Backup) -> Result<(), TransactionFileSystemError> {
        Ok(())
    }

    async fn sync_journal(
        &self,
        journal: &Self::Journal,
    ) -> Result<(), TransactionFileSystemError> {
        let _ = (&journal.transaction_id, journal.generation);
        Ok(())
    }

    async fn sync_storage(
        &self,
        _storage: &Self::Storage,
    ) -> Result<(), TransactionFileSystemError> {
        let mut state = self.state.lock().unwrap();
        let persist_calls = state.persist_calls;
        if state.crash_after_persist_at == Some(state.persist_calls) {
            state.crash_after_persist_at = None;
            drop(state);
            panic!("injected crash after durable journal persist");
        }
        drop(state);
        let mut pause = self.persist_pause.0.lock().unwrap();
        if pause.persist_call == Some(persist_calls) {
            pause.reached = true;
            self.persist_pause.1.notify_all();
            while !pause.released {
                pause = self.persist_pause.1.wait(pause).unwrap();
            }
        }
        Ok(())
    }

    async fn cleanup_artifacts(
        &self,
        storage: &Self::Storage,
    ) -> Result<(), TransactionFileSystemError> {
        let mut state = self.state.lock().unwrap();
        state
            .staged
            .retain(|_, staged| staged.transaction_id != storage.transaction_id);
        state
            .backups
            .retain(|_, backup| backup.transaction_id != storage.transaction_id);
        state.events.push(TestEvent::Cleaned);
        if state.crash_after_cleanup {
            state.crash_after_cleanup = false;
            drop(state);
            panic!("injected crash after cleanup");
        }
        Ok(())
    }
}

impl TransactionMutation for TestFileSystem {
    async fn apply_guarded(
        &self,
        _lease: &Self::Lease,
        _journal: &Self::Journal,
        mutation: GuardedMutation<'_, Self::ResolvedPath, Self::StagedFile, Self::Backup>,
    ) -> Result<MutationOutcome, TransactionFileSystemError> {
        let mut state = self.state.lock().unwrap();
        state.apply_calls += 1;
        if state.fail_apply_at == Some(state.apply_calls) {
            state.fail_apply_at = None;
            return Err(platform("apply guarded", "injected failure"));
        }
        let (label, outcome) = apply(&mut state, mutation)?;
        state.events.push(TestEvent::Applied(label));
        if state.crash_after_apply_at == Some(state.apply_calls) {
            state.crash_after_apply_at = None;
            drop(state);
            panic!("injected crash after apply guarded");
        }
        Ok(outcome)
    }

    async fn restore_guarded(
        &self,
        _lease: &Self::Lease,
        _journal: &Self::Journal,
        rollback: GuardedRollback<'_, Self::ResolvedPath, Self::StagedFile, Self::Backup>,
    ) -> Result<MutationOutcome, TransactionFileSystemError> {
        let mut state = self.state.lock().unwrap();
        state.restore_calls += 1;
        if state.fail_restore_at == Some(state.restore_calls) {
            state.fail_restore_at = None;
            return Err(platform("restore guarded", "injected failure"));
        }
        let (label, outcome) = restore(&mut state, rollback)?;
        state.events.push(TestEvent::Restored(label));
        if state.crash_after_restore_at == Some(state.restore_calls) {
            state.crash_after_restore_at = None;
            drop(state);
            panic!("injected crash after restore guarded");
        }
        Ok(outcome)
    }

    async fn sync_parent(
        &self,
        _path: &Self::ResolvedPath,
    ) -> Result<(), TransactionFileSystemError> {
        Ok(())
    }

    async fn restore_metadata(
        &self,
        path: &Self::ResolvedPath,
        metadata: &MetadataSnapshot,
    ) -> Result<(), TransactionFileSystemError> {
        let mut state = self.state.lock().unwrap();
        let file = state
            .files
            .get_mut(path)
            .ok_or_else(|| platform("restore metadata", format!("missing {path}")))?;
        file.metadata = metadata.clone();
        Ok(())
    }
}

pub(super) fn observe(state: &TestState, path: &str) -> ObservedPath {
    state
        .files
        .get(path)
        .cloned()
        .map_or(ObservedPath::Absent, ObservedPath::Present)
}

pub(super) fn durable_evidence(key: &str, file: &ObservedFile) -> DurableFileEvidence {
    DurableFileEvidence {
        key: durable_key("test-artifact", key),
        evidence: FileEvidence::from(file),
    }
}

fn durable_key(namespace: &str, value: &str) -> DurablePathKey {
    DurablePathKey {
        namespace: namespace.to_string(),
        value: value.as_bytes().to_vec(),
    }
}

pub(super) fn changed(path: &str) -> TransactionFileSystemError {
    TransactionFileSystemError::ChangedSincePlanning {
        path: path.to_string(),
    }
}

pub(super) fn platform(
    operation: &'static str,
    reason: impl Into<String>,
) -> TransactionFileSystemError {
    TransactionFileSystemError::Platform {
        operation,
        reason: reason.into(),
    }
}
