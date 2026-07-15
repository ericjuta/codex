use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::Mutex;

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

#[derive(Clone, Debug)]
pub(super) struct TestFileSystem {
    pub(super) state: Arc<Mutex<TestState>>,
}

#[derive(Debug, Default)]
pub(super) struct TestState {
    files: BTreeMap<String, ObservedFile>,
    pub(super) staged: BTreeMap<String, TestStagedFile>,
    pub(super) backups: BTreeMap<String, TestBackup>,
    pub(super) journals: Vec<JournalRecord>,
    events: Vec<TestEvent>,
    next_artifact: usize,
    persist_calls: usize,
    apply_calls: usize,
    restore_calls: usize,
    fail_persist_at: Option<usize>,
    fail_apply_at: Option<usize>,
    fail_restore_at: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum TestEvent {
    Locked(Vec<String>),
    Applied(String),
    Restored(String),
    Cleaned,
}

#[derive(Clone, Debug)]
pub(super) struct TestLease;

#[derive(Clone, Debug)]
pub(super) struct TestStorage {
    pub(super) transaction_id: TransactionId,
}

#[derive(Clone, Debug)]
pub(super) struct TestStagedFile {
    key: String,
    destination: String,
    pub(super) file: ObservedFile,
}

#[derive(Clone, Debug)]
pub(super) struct TestBackup {
    key: String,
    source: String,
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
        _root: &Self::Root,
        paths: &[Self::ResolvedPath],
    ) -> Result<Self::Lease, TransactionFileSystemError> {
        self.state
            .lock()
            .unwrap()
            .events
            .push(TestEvent::Locked(paths.to_vec()));
        Ok(TestLease)
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
        Ok(TestStorage {
            transaction_id: transaction_id.clone(),
        })
    }

    async fn stage_file(
        &self,
        _storage: &Self::Storage,
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
        _storage: &Self::Storage,
        source: &Self::ResolvedPath,
        expected: &FileEvidence,
    ) -> Result<Self::Backup, TransactionFileSystemError> {
        let mut state = self.state.lock().unwrap();
        require_file(&state, source, expected, "backup file")?;
        let file = state.files[source].clone();
        let index = state.next_artifact;
        state.next_artifact += 1;
        let key = format!("backup-{index}");
        let backup = TestBackup {
            key: key.clone(),
            source: source.clone(),
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
        Ok(())
    }

    async fn cleanup_artifacts(
        &self,
        _storage: &Self::Storage,
    ) -> Result<(), TransactionFileSystemError> {
        let mut state = self.state.lock().unwrap();
        state.staged.clear();
        state.backups.clear();
        state.events.push(TestEvent::Cleaned);
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

fn require_file(
    state: &TestState,
    path: &str,
    expected: &FileEvidence,
    operation: &'static str,
) -> Result<(), TransactionFileSystemError> {
    if state
        .files
        .get(path)
        .is_some_and(|file| FileEvidence::from(file) == *expected)
    {
        Ok(())
    } else {
        Err(TransactionFileSystemError::ChangedSincePlanning {
            path: format!("{operation}:{path}"),
        })
    }
}

fn apply(
    state: &mut TestState,
    mutation: GuardedMutation<'_, String, TestStagedFile, TestBackup>,
) -> Result<(String, MutationOutcome), TransactionFileSystemError> {
    match mutation {
        GuardedMutation::Create {
            destination,
            staged,
        } => {
            require_destination(staged, destination)?;
            if state.files.get(destination) == Some(&staged.file) {
                return Ok((
                    format!("create:{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            if state.files.contains_key(destination) {
                return Err(changed(destination));
            }
            state.files.insert(destination.clone(), staged.file.clone());
            Ok((format!("create:{destination}"), MutationOutcome::Applied))
        }
        GuardedMutation::Replace {
            destination,
            expected,
            staged,
            backup,
        } => {
            require_destination(staged, destination)?;
            require_backup(backup, destination, expected)?;
            if state.files.get(destination) == Some(&staged.file) {
                return Ok((
                    format!("replace:{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            require_file(state, destination, expected, "replace")?;
            state.files.insert(destination.clone(), staged.file.clone());
            Ok((format!("replace:{destination}"), MutationOutcome::Applied))
        }
        GuardedMutation::Remove {
            source,
            expected,
            backup,
        } => {
            require_backup(backup, source, expected)?;
            if !state.files.contains_key(source) {
                return Ok((format!("remove:{source}"), MutationOutcome::AlreadyApplied));
            }
            require_file(state, source, expected, "remove")?;
            state.files.remove(source);
            Ok((format!("remove:{source}"), MutationOutcome::Applied))
        }
        GuardedMutation::Move {
            source,
            expected,
            destination,
            staged,
            backup,
        } => {
            let staged = staged.ok_or_else(|| platform("move", "missing staged after-image"))?;
            require_destination(staged, destination)?;
            require_backup(backup, source, expected)?;
            if !state.files.contains_key(source)
                && state.files.get(destination) == Some(&staged.file)
            {
                return Ok((
                    format!("move:{source}->{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            require_file(state, source, expected, "move")?;
            if state.files.contains_key(destination) {
                return Err(changed(destination));
            }
            state.files.remove(source);
            state.files.insert(destination.clone(), staged.file.clone());
            Ok((
                format!("move:{source}->{destination}"),
                MutationOutcome::Applied,
            ))
        }
    }
}

fn restore(
    state: &mut TestState,
    rollback: GuardedRollback<'_, String, TestStagedFile, TestBackup>,
) -> Result<(String, MutationOutcome), TransactionFileSystemError> {
    match rollback {
        GuardedRollback::RemoveCreated {
            destination,
            staged,
        } => {
            require_destination(staged, destination)?;
            if !state.files.contains_key(destination) {
                return Ok((
                    format!("create:{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            require_file(
                state,
                destination,
                &FileEvidence::from(&staged.file),
                "rollback create",
            )?;
            state.files.remove(destination);
            Ok((format!("create:{destination}"), MutationOutcome::Applied))
        }
        GuardedRollback::RestoreReplaced {
            destination,
            staged,
            backup,
        } => {
            require_destination(staged, destination)?;
            require_backup_source(backup, destination)?;
            if state.files.get(destination) == Some(&backup.file) {
                return Ok((
                    format!("replace:{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            require_file(
                state,
                destination,
                &FileEvidence::from(&staged.file),
                "rollback replace",
            )?;
            state.files.insert(destination.clone(), backup.file.clone());
            Ok((format!("replace:{destination}"), MutationOutcome::Applied))
        }
        GuardedRollback::RestoreRemoved { source, backup } => {
            require_backup_source(backup, source)?;
            if state.files.get(source) == Some(&backup.file) {
                return Ok((format!("remove:{source}"), MutationOutcome::AlreadyApplied));
            }
            if state.files.contains_key(source) {
                return Err(changed(source));
            }
            state.files.insert(source.clone(), backup.file.clone());
            Ok((format!("remove:{source}"), MutationOutcome::Applied))
        }
        GuardedRollback::RestoreMove {
            source,
            destination,
            staged,
            backup,
        } => {
            require_destination(staged, destination)?;
            require_backup_source(backup, source)?;
            if state.files.get(source) == Some(&backup.file)
                && !state.files.contains_key(destination)
            {
                return Ok((
                    format!("move:{source}->{destination}"),
                    MutationOutcome::AlreadyApplied,
                ));
            }
            if state.files.contains_key(source) {
                return Err(changed(source));
            }
            require_file(
                state,
                destination,
                &FileEvidence::from(&staged.file),
                "rollback move",
            )?;
            state.files.remove(destination);
            state.files.insert(source.clone(), backup.file.clone());
            Ok((
                format!("move:{source}->{destination}"),
                MutationOutcome::Applied,
            ))
        }
    }
}

fn require_destination(
    staged: &TestStagedFile,
    destination: &str,
) -> Result<(), TransactionFileSystemError> {
    if staged.destination == destination {
        Ok(())
    } else {
        Err(platform("staged destination", destination.to_string()))
    }
}

fn require_backup(
    backup: &TestBackup,
    source: &str,
    expected: &FileEvidence,
) -> Result<(), TransactionFileSystemError> {
    require_backup_source(backup, source)?;
    if FileEvidence::from(&backup.file) == *expected {
        Ok(())
    } else {
        Err(changed(source))
    }
}

fn require_backup_source(
    backup: &TestBackup,
    source: &str,
) -> Result<(), TransactionFileSystemError> {
    if backup.source == source {
        Ok(())
    } else {
        Err(platform("backup source", source.to_string()))
    }
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
