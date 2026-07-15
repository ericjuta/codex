use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::io::Write;
use std::num::NonZeroU64;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;

use codex_hashline_transaction::DurableFileEvidence;
use codex_hashline_transaction::DurablePathKey;
use codex_hashline_transaction::DurableTransactionKey;
use codex_hashline_transaction::FileEvidence;
use codex_hashline_transaction::FileKind;
use codex_hashline_transaction::JournalBytes;
use codex_hashline_transaction::ObservationLimit;
use codex_hashline_transaction::ObservedPath;
use codex_hashline_transaction::StageFileRequest;
use codex_hashline_transaction::StorageRequirements;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_hashline_transaction::TransactionId;
use codex_hashline_transaction::TransactionStorage;

use super::NativeResolvedPath;
use super::NativeRoot;
use super::file_kind;
use super::observe;
use super::run_blocking;
use super::storage_io::apply_staged_metadata;
use super::storage_io::artifact_evidence;
use super::storage_io::artifact_name;
use super::storage_io::changed_since_planning;
use super::storage_io::create_directory;
use super::storage_io::create_exclusive_file;
use super::storage_io::open_internal_directory;
use super::storage_io::open_internal_file_read_write;
use super::storage_io::open_or_create_sidecar;
use super::storage_io::platform_error;
use super::storage_io::push_key_part;
use super::storage_io::remove_directory_contents;
use super::storage_io::rename_at;
use super::storage_io::require_storage_root;
use super::storage_io::reserve_capacity;
use super::storage_io::storage_error;
use super::storage_io::transaction_key_value;
use super::storage_io::unlink_at;
use super::storage_io::validate_internal_directory;
use super::storage_io::validate_transaction_id;
use crate::hashline_transaction_fs::NativeTransactionFileSystem;

const STAGED_DIRECTORY_NAME: &[u8] = b"staged";
const BACKUP_DIRECTORY_NAME: &[u8] = b"backups";
const RESERVATION_FILE_NAME: &[u8] = b"reservation";
const JOURNAL_FILE_NAME: &[u8] = b"journal";
const ROOT_KEY_NAMESPACE: &str = "linux-hashline-root-v1";
const PATH_KEY_NAMESPACE: &str = "linux-hashline-path-v1";
const TRANSACTION_KEY_NAMESPACE: &str = "linux-hashline-transaction-v1";

#[derive(Clone, Debug)]
pub struct NativeStorage {
    pub(super) root: NativeRoot,
    pub(super) sidecar: Arc<File>,
    pub(super) directory: Arc<File>,
    pub(super) staged_directory: Arc<File>,
    pub(super) backup_directory: Arc<File>,
    pub(super) _reservation: Arc<File>,
    pub(super) transaction_id: TransactionId,
    requirements: StorageRequirements,
    budget: Arc<Mutex<StorageBudget>>,
    next_artifact: Arc<AtomicU64>,
}

#[derive(Clone, Debug)]
pub struct NativeStagedFile {
    pub(super) parent: Arc<File>,
    pub(super) name: OsString,
    pub(super) file: Arc<File>,
    pub(super) evidence: DurableFileEvidence,
}

#[derive(Clone, Debug)]
pub struct NativeBackup {
    pub(super) parent: Arc<File>,
    pub(super) name: OsString,
    pub(super) file: Arc<File>,
    pub(super) evidence: DurableFileEvidence,
}

#[derive(Clone, Debug)]
pub struct NativeJournal {
    pub(super) directory: Arc<File>,
    pub(super) file: Arc<File>,
}

#[derive(Clone, Copy, Debug)]
struct StorageBudget {
    staged_remaining: u64,
    backup_remaining: u64,
}

impl TransactionStorage for NativeTransactionFileSystem {
    type Storage = NativeStorage;
    type StagedFile = NativeStagedFile;
    type Backup = NativeBackup;
    type Journal = NativeJournal;

    async fn allocate_storage(
        &self,
        lease: &Self::Lease,
        transaction_id: &TransactionId,
        requirements: StorageRequirements,
    ) -> Result<Self::Storage, TransactionFileSystemError> {
        let root = lease.root().clone();
        let transaction_id = transaction_id.clone();
        run_blocking("allocate transaction storage", move || {
            allocate_storage(root, transaction_id, requirements)
        })
        .await
    }

    async fn stage_file(
        &self,
        storage: &Self::Storage,
        request: StageFileRequest<'_, Self::ResolvedPath>,
    ) -> Result<Self::StagedFile, TransactionFileSystemError> {
        let storage = storage.clone();
        let destination = request.destination.clone();
        let contents = request.contents.to_vec();
        let metadata = request.metadata.cloned();
        run_blocking("stage transaction file", move || {
            stage_file(&storage, &destination, &contents, metadata.as_ref())
        })
        .await
    }

    async fn backup_file(
        &self,
        storage: &Self::Storage,
        source: &Self::ResolvedPath,
        expected: &FileEvidence,
    ) -> Result<Self::Backup, TransactionFileSystemError> {
        let storage = storage.clone();
        let source = source.clone();
        let expected = expected.clone();
        run_blocking("backup transaction file", move || {
            backup_file(&storage, &source, &expected)
        })
        .await
    }

    fn staged_file_evidence(
        &self,
        staged: &Self::StagedFile,
    ) -> Result<DurableFileEvidence, TransactionFileSystemError> {
        Ok(staged.evidence.clone())
    }

    fn backup_evidence(
        &self,
        backup: &Self::Backup,
    ) -> Result<DurableFileEvidence, TransactionFileSystemError> {
        Ok(backup.evidence.clone())
    }

    fn durable_root_key(
        &self,
        root: &Self::Root,
    ) -> Result<DurablePathKey, TransactionFileSystemError> {
        Ok(DurablePathKey {
            namespace: ROOT_KEY_NAMESPACE.to_string(),
            value: root.identity.value.clone(),
        })
    }

    fn durable_path_key(
        &self,
        path: &Self::ResolvedPath,
    ) -> Result<DurablePathKey, TransactionFileSystemError> {
        let mut value = Vec::new();
        push_key_part(&mut value, &path.root_identity.value)?;
        for edge in &path.directory_chain {
            push_key_part(&mut value, edge.component.as_bytes())?;
        }
        push_key_part(&mut value, path.final_component.as_bytes())?;
        Ok(DurablePathKey {
            namespace: PATH_KEY_NAMESPACE.to_string(),
            value,
        })
    }

    fn durable_transaction_key(
        &self,
        storage: &Self::Storage,
    ) -> Result<DurableTransactionKey, TransactionFileSystemError> {
        Ok(DurableTransactionKey {
            namespace: TRANSACTION_KEY_NAMESPACE.to_string(),
            value: transaction_key_value(&storage.root, &storage.transaction_id)?,
        })
    }

    async fn persist_journal(
        &self,
        storage: &Self::Storage,
        journal: &JournalBytes,
    ) -> Result<Self::Journal, TransactionFileSystemError> {
        let storage = storage.clone();
        let bytes = journal.as_bytes().to_vec();
        run_blocking("persist transaction journal", move || {
            persist_journal(&storage, &bytes)
        })
        .await
    }

    async fn sync_staged_file(
        &self,
        staged: &Self::StagedFile,
    ) -> Result<(), TransactionFileSystemError> {
        let file = Arc::clone(&staged.file);
        run_blocking("sync staged transaction file", move || sync_file(&file)).await
    }

    async fn sync_backup(&self, backup: &Self::Backup) -> Result<(), TransactionFileSystemError> {
        let file = Arc::clone(&backup.file);
        run_blocking("sync transaction backup", move || sync_file(&file)).await
    }

    async fn sync_journal(
        &self,
        journal: &Self::Journal,
    ) -> Result<(), TransactionFileSystemError> {
        let file = Arc::clone(&journal.file);
        run_blocking("sync transaction journal", move || sync_file(&file)).await
    }

    async fn sync_storage(
        &self,
        storage: &Self::Storage,
    ) -> Result<(), TransactionFileSystemError> {
        let storage = storage.clone();
        run_blocking("sync transaction storage", move || sync_storage(&storage)).await
    }

    async fn cleanup_artifacts(
        &self,
        storage: &Self::Storage,
    ) -> Result<(), TransactionFileSystemError> {
        let storage = storage.clone();
        run_blocking("cleanup transaction artifacts", move || {
            cleanup_artifacts(&storage)
        })
        .await
    }
}

/// Reopens validated durable storage while recovery owns the transaction lease.
pub(super) fn reopen_storage(
    root: NativeRoot,
    transaction_id: TransactionId,
) -> Result<NativeStorage, TransactionFileSystemError> {
    validate_transaction_id(&transaction_id)?;
    let sidecar = Arc::new(open_or_create_sidecar(&root)?);
    let transaction_name = OsStr::new(&transaction_id.0);
    let directory = Arc::new(open_internal_directory(&sidecar, transaction_name)?);
    validate_internal_directory(&root, &directory)?;
    let staged_directory = Arc::new(open_internal_directory(
        &directory,
        OsStr::from_bytes(STAGED_DIRECTORY_NAME),
    )?);
    let backup_directory = Arc::new(open_internal_directory(
        &directory,
        OsStr::from_bytes(BACKUP_DIRECTORY_NAME),
    )?);
    validate_internal_directory(&root, &staged_directory)?;
    validate_internal_directory(&root, &backup_directory)?;
    let reservation = open_internal_file_read_write(
        &directory,
        OsStr::from_bytes(RESERVATION_FILE_NAME),
        "open transaction reservation",
    )?;
    let reservation_metadata = reservation
        .metadata()
        .map_err(|error| platform_error("inspect transaction reservation", error))?;
    let root_metadata = root
        .directory
        .metadata()
        .map_err(|error| platform_error("inspect transaction root", error))?;
    if file_kind(&reservation_metadata) != FileKind::File
        || reservation_metadata.nlink() != 1
        || reservation_metadata.dev() != root_metadata.dev()
        || reservation_metadata.uid() != unsafe { libc::geteuid() }
        || reservation_metadata.mode() & 0o077 != 0
    {
        return Err(storage_error(
            "transaction reservation is not a private same-filesystem regular file",
        ));
    }
    let requirements = StorageRequirements {
        staged_bytes: u64::MAX,
        backup_bytes: u64::MAX,
        journal_bytes: u64::MAX,
    };
    Ok(NativeStorage {
        root,
        sidecar,
        directory,
        staged_directory,
        backup_directory,
        _reservation: Arc::new(reservation),
        transaction_id,
        requirements,
        budget: Arc::new(Mutex::new(StorageBudget {
            staged_remaining: u64::MAX,
            backup_remaining: u64::MAX,
        })),
        next_artifact: Arc::new(AtomicU64::new(0)),
    })
}

fn allocate_storage(
    root: NativeRoot,
    transaction_id: TransactionId,
    requirements: StorageRequirements,
) -> Result<NativeStorage, TransactionFileSystemError> {
    validate_transaction_id(&transaction_id)?;
    let total_bytes = requirements
        .staged_bytes
        .checked_add(requirements.backup_bytes)
        .and_then(|total| total.checked_add(requirements.journal_bytes))
        .ok_or_else(|| storage_error("storage requirements overflow"))?;
    let reserved_bytes = i64::try_from(total_bytes)
        .map_err(|_| storage_error("storage requirements exceed Linux file-size limits"))?;
    let sidecar = Arc::new(open_or_create_sidecar(&root)?);
    let transaction_name = OsStr::new(&transaction_id.0);
    create_directory(&sidecar, transaction_name, "allocate transaction storage")?;
    let directory = Arc::new(open_internal_directory(&sidecar, transaction_name)?);
    validate_internal_directory(&root, &directory)?;
    create_directory(
        &directory,
        OsStr::from_bytes(STAGED_DIRECTORY_NAME),
        "create staged directory",
    )?;
    create_directory(
        &directory,
        OsStr::from_bytes(BACKUP_DIRECTORY_NAME),
        "create backup directory",
    )?;
    let staged_directory = Arc::new(open_internal_directory(
        &directory,
        OsStr::from_bytes(STAGED_DIRECTORY_NAME),
    )?);
    let backup_directory = Arc::new(open_internal_directory(
        &directory,
        OsStr::from_bytes(BACKUP_DIRECTORY_NAME),
    )?);
    validate_internal_directory(&root, &staged_directory)?;
    validate_internal_directory(&root, &backup_directory)?;
    let reservation = Arc::new(create_exclusive_file(
        &directory,
        OsStr::from_bytes(RESERVATION_FILE_NAME),
        "reserve transaction capacity",
    )?);
    lock_storage(&reservation)?;
    reserve_capacity(&reservation, reserved_bytes)?;
    reservation
        .sync_all()
        .map_err(|error| platform_error("sync transaction reservation", error))?;
    directory
        .sync_all()
        .map_err(|error| platform_error("sync transaction directory", error))?;
    sidecar
        .sync_all()
        .map_err(|error| platform_error("sync transaction sidecar", error))?;
    root.directory
        .sync_all()
        .map_err(|error| platform_error("sync transaction root", error))?;
    Ok(NativeStorage {
        root,
        sidecar,
        directory,
        staged_directory,
        backup_directory,
        _reservation: reservation,
        transaction_id,
        requirements,
        budget: Arc::new(Mutex::new(StorageBudget {
            staged_remaining: requirements.staged_bytes,
            backup_remaining: requirements.backup_bytes,
        })),
        next_artifact: Arc::new(AtomicU64::new(0)),
    })
}

fn stage_file(
    storage: &NativeStorage,
    destination: &NativeResolvedPath,
    contents: &[u8],
    metadata: Option<&codex_hashline_transaction::MetadataSnapshot>,
) -> Result<NativeStagedFile, TransactionFileSystemError> {
    require_storage_root(storage, destination, "stage transaction file")?;
    let byte_count = contents.len() as u64;
    let mut budget = storage
        .budget
        .lock()
        .map_err(|_| storage_error("transaction storage budget lock is poisoned"))?;
    if byte_count > budget.staged_remaining {
        return Err(storage_error(
            "staged bytes exceed the reserved transaction budget",
        ));
    }
    let name = artifact_name("staged", &storage.next_artifact);
    let mut file =
        create_exclusive_file(&storage.staged_directory, &name, "stage transaction file")?;
    file.write_all(contents)
        .map_err(|error| platform_error("write staged transaction file", error))?;
    apply_staged_metadata(&file, metadata)?;
    let evidence = artifact_evidence(storage, b"staged", &name, &file, contents)?;
    budget.staged_remaining -= byte_count;
    Ok(NativeStagedFile {
        parent: Arc::clone(&storage.staged_directory),
        name,
        file: Arc::new(file),
        evidence,
    })
}

fn backup_file(
    storage: &NativeStorage,
    source: &NativeResolvedPath,
    expected: &FileEvidence,
) -> Result<NativeBackup, TransactionFileSystemError> {
    require_storage_root(storage, source, "backup transaction file")?;
    if expected.kind != FileKind::File || expected.link_count != NonZeroU64::MIN {
        return Err(TransactionFileSystemError::Unsupported {
            capability: "transaction backup",
            reason: "native backup currently requires a singly linked regular file".to_string(),
        });
    }
    let mut budget = storage
        .budget
        .lock()
        .map_err(|_| storage_error("transaction storage budget lock is poisoned"))?;
    let observed = observe(
        source,
        ObservationLimit {
            max_bytes: budget.backup_remaining,
        },
    )?;
    let ObservedPath::Present(observed) = observed else {
        return Err(changed_since_planning(source));
    };
    if FileEvidence::from(&observed) != *expected {
        return Err(changed_since_planning(source));
    }
    let byte_count = observed.contents.len() as u64;
    let name = artifact_name("backup", &storage.next_artifact);
    let mut file =
        create_exclusive_file(&storage.backup_directory, &name, "backup transaction file")?;
    file.write_all(&observed.contents)
        .map_err(|error| platform_error("write transaction backup", error))?;
    apply_staged_metadata(&file, Some(&expected.metadata))?;
    let evidence = artifact_evidence(storage, b"backup", &name, &file, &observed.contents)?;
    budget.backup_remaining -= byte_count;
    Ok(NativeBackup {
        parent: Arc::clone(&storage.backup_directory),
        name,
        file: Arc::new(file),
        evidence,
    })
}

fn persist_journal(
    storage: &NativeStorage,
    bytes: &[u8],
) -> Result<NativeJournal, TransactionFileSystemError> {
    if bytes.len() as u64 > storage.requirements.journal_bytes {
        return Err(storage_error(
            "journal bytes exceed the reserved transaction budget",
        ));
    }
    let temporary_name = artifact_name("journal-tmp", &storage.next_artifact);
    let mut file = create_exclusive_file(
        &storage.directory,
        &temporary_name,
        "create transaction journal temporary",
    )?;
    file.write_all(bytes)
        .map_err(|error| platform_error("write transaction journal", error))?;
    file.sync_all()
        .map_err(|error| platform_error("sync transaction journal temporary", error))?;
    let journal_name = OsStr::from_bytes(JOURNAL_FILE_NAME);
    if let Err(error) = rename_at(
        &storage.directory,
        &temporary_name,
        &storage.directory,
        journal_name,
    ) {
        let _ = unlink_at(&storage.directory, &temporary_name);
        return Err(error);
    }
    storage
        .directory
        .sync_all()
        .map_err(|error| platform_error("sync transaction journal directory", error))?;
    Ok(NativeJournal {
        directory: Arc::clone(&storage.directory),
        file: Arc::new(file),
    })
}

fn sync_file(file: &File) -> Result<(), TransactionFileSystemError> {
    file.sync_all()
        .map_err(|error| platform_error("sync transaction file", error))
}

fn sync_storage(storage: &NativeStorage) -> Result<(), TransactionFileSystemError> {
    for directory in [
        &storage.staged_directory,
        &storage.backup_directory,
        &storage.directory,
        &storage.sidecar,
        &storage.root.directory,
    ] {
        directory
            .sync_all()
            .map_err(|error| platform_error("sync transaction storage", error))?;
    }
    Ok(())
}

fn cleanup_artifacts(storage: &NativeStorage) -> Result<(), TransactionFileSystemError> {
    remove_directory_contents(&storage.staged_directory)?;
    remove_directory_contents(&storage.backup_directory)?;
    storage
        ._reservation
        .set_len(0)
        .map_err(|error| platform_error("release transaction reservation", error))?;
    storage
        ._reservation
        .sync_all()
        .map_err(|error| platform_error("sync transaction reservation", error))?;
    sync_storage(storage)
}

fn lock_storage(file: &File) -> Result<(), TransactionFileSystemError> {
    loop {
        // SAFETY: `file` owns a live descriptor and `NativeStorage` retains it for the
        // transaction lifetime, so the lock is released only after commit or recovery exits.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(platform_error("lock transaction storage", error));
        }
    }
}

#[cfg(test)]
#[path = "hashline_transaction_fs_linux_storage_tests.rs"]
mod tests;
