use std::ffi::CStr;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;

use codex_hashline_transaction::DurablePathKey;
use codex_hashline_transaction::DurableTransactionKey;
use codex_hashline_transaction::FileKind;
use codex_hashline_transaction::JournalBytes;
use codex_hashline_transaction::JournalReadLimits;
use codex_hashline_transaction::JournalRecord;
use codex_hashline_transaction::JournalState;
use codex_hashline_transaction::LoadedJournal;
use codex_hashline_transaction::RecoveryScanLimit;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_hashline_transaction::TransactionLimits;

use super::NativeRoot;
use super::OpenKind;
use super::TRANSACTION_SIDECAR_NAME;
use super::entry_is_symlink;
use super::file_kind;
use super::metadata_is_stable;
use super::open_component;
use super::storage::NativeJournal;
use super::storage_io::open_internal_directory;
use super::storage_io::platform_error;
use super::storage_io::push_key_part;
use super::storage_io::validate_internal_directory;

const ROOT_KEY_NAMESPACE: &str = "linux-hashline-root-v1";
const TRANSACTION_KEY_NAMESPACE: &str = "linux-hashline-transaction-v1";
const MAX_RECOVERY_SCAN_ENTRIES: u64 = 4096;

pub(super) fn pending_recovery_keys(
    root: &NativeRoot,
    environment_id: &str,
    limit: RecoveryScanLimit,
) -> Result<Vec<DurableTransactionKey>, TransactionFileSystemError> {
    let Some(sidecar) = open_sidecar(root)? else {
        return Ok(Vec::new());
    };
    let names = directory_names(&sidecar, MAX_RECOVERY_SCAN_ENTRIES)?;
    let root_key = DurablePathKey {
        namespace: ROOT_KEY_NAMESPACE.to_string(),
        value: root.identity.value.clone(),
    };
    let mut keys = Vec::new();
    for name in names {
        let key = transaction_key_for_name(root, &name)?;
        let needs_recovery = match open_internal_directory(&sidecar, &name) {
            Ok(directory) => match journal_record_if_present(Arc::new(directory)) {
                Ok(None) => false,
                Ok(Some(record)) => {
                    record.state != JournalState::Complete
                        || record.environment_id != environment_id
                        || record.root != root_key
                        || record.root_identity != root.identity
                        || record.transaction_key != key
                        || record.transaction_id.0.as_bytes() != name.as_bytes()
                }
                Err(_) => true,
            },
            Err(_) => true,
        };
        if needs_recovery {
            keys.push(key);
            if keys.len() as u64 > limit.max_transactions {
                break;
            }
        }
    }
    Ok(keys)
}

pub(super) fn load_journal_from_directory(
    directory: Arc<File>,
    max_bytes: u64,
) -> Result<LoadedJournal<NativeJournal>, TransactionFileSystemError> {
    let file = open_journal(&directory)?.ok_or_else(|| TransactionFileSystemError::Platform {
        operation: "load transaction journal",
        reason: "transaction journal is missing".to_string(),
    })?;
    load_open_journal(directory, file, max_bytes)
}

fn open_sidecar(root: &NativeRoot) -> Result<Option<File>, TransactionFileSystemError> {
    let name = OsStr::from_bytes(TRANSACTION_SIDECAR_NAME);
    match open_component(&root.directory, name, OpenKind::Directory) {
        Ok(directory) => {
            validate_internal_directory(root, &directory)?;
            Ok(Some(directory))
        }
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                && !entry_is_symlink(&root.directory, name) =>
        {
            Ok(None)
        }
        Err(_) if entry_is_symlink(&root.directory, name) => {
            Err(TransactionFileSystemError::SymbolicLink {
                path: root.absolute.as_path().join(name).display().to_string(),
            })
        }
        Err(error) => Err(platform_error("open transaction recovery sidecar", error)),
    }
}

fn directory_names(
    directory: &File,
    max_entries: u64,
) -> Result<Vec<OsString>, TransactionFileSystemError> {
    // SAFETY: `directory` owns a live descriptor.
    let duplicated = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicated < 0 {
        return Err(platform_error(
            "enumerate transaction recovery storage",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: `duplicated` is independently owned and transferred to `fdopendir`.
    let stream = unsafe { libc::fdopendir(duplicated) };
    if stream.is_null() {
        // SAFETY: `fdopendir` did not take ownership on failure.
        unsafe { libc::close(duplicated) };
        return Err(platform_error(
            "enumerate transaction recovery storage",
            io::Error::last_os_error(),
        ));
    }
    let mut names = Vec::new();
    loop {
        // SAFETY: this module only builds for Linux and clears thread-local errno before `readdir`.
        unsafe { *libc::__errno_location() = 0 };
        // SAFETY: `stream` remains live until the matching `closedir` below.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            // SAFETY: errno is thread-local on Linux and was cleared immediately before `readdir`.
            let errno = unsafe { *libc::__errno_location() };
            // SAFETY: `stream` is live and closed exactly once.
            unsafe { libc::closedir(stream) };
            if errno == 0 {
                break;
            }
            return Err(platform_error(
                "enumerate transaction recovery storage",
                io::Error::from_raw_os_error(errno),
            ));
        }
        // SAFETY: `readdir` returned an initialized entry with a NUL-terminated name.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        names.push(OsString::from_vec(name.to_vec()));
        if names.len() as u64 > max_entries {
            // SAFETY: `stream` is live and closed exactly once.
            unsafe { libc::closedir(stream) };
            return Err(recovery_error(
                "enumerate transaction recovery storage",
                "transaction recovery sidecar exceeds the hard entry limit",
            ));
        }
    }
    names.sort();
    Ok(names)
}

fn journal_record_if_present(
    directory: Arc<File>,
) -> Result<Option<JournalRecord>, TransactionFileSystemError> {
    let Some(file) = open_journal(&directory)? else {
        return Ok(None);
    };
    let limits = TransactionLimits::default();
    let loaded = load_open_journal(directory, file, limits.max_journal_bytes)?;
    let record =
        JournalRecord::from_bounded_json(&loaded.bytes, JournalReadLimits::from(limits))
            .map_err(|error| recovery_error("decode transaction journal", &error.to_string()))?;
    Ok(Some(record))
}

fn open_journal(directory: &File) -> Result<Option<File>, TransactionFileSystemError> {
    let name = OsStr::new("journal");
    match open_component(directory, name, OpenKind::Any) {
        Ok(file) => Ok(Some(file)),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound && !entry_is_symlink(directory, name) =>
        {
            Ok(None)
        }
        Err(_) if entry_is_symlink(directory, name) => {
            Err(TransactionFileSystemError::SymbolicLink {
                path: "transaction recovery journal".to_string(),
            })
        }
        Err(error) => Err(platform_error("open transaction journal", error)),
    }
}

fn load_open_journal(
    directory: Arc<File>,
    file: File,
    max_bytes: u64,
) -> Result<LoadedJournal<NativeJournal>, TransactionFileSystemError> {
    let before = file
        .metadata()
        .map_err(|error| platform_error("inspect transaction journal", error))?;
    let directory_metadata = directory
        .metadata()
        .map_err(|error| platform_error("inspect transaction directory", error))?;
    if file_kind(&before) != FileKind::File
        || before.nlink() != 1
        || before.dev() != directory_metadata.dev()
        || before.uid() != unsafe { libc::geteuid() }
        || before.mode() & 0o077 != 0
    {
        return Err(recovery_error(
            "load transaction journal",
            "transaction journal is not a private same-filesystem regular file",
        ));
    }
    if before.size() > max_bytes {
        return Err(recovery_error(
            "load transaction journal",
            "transaction journal exceeds the configured recovery limit",
        ));
    }
    let capacity = usize::try_from(before.size().min(max_bytes)).unwrap_or(usize::MAX);
    let mut bytes = Vec::with_capacity(capacity);
    file.try_clone()
        .map_err(|error| platform_error("clone transaction journal", error))?
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| platform_error("read transaction journal", error))?;
    let after = file
        .metadata()
        .map_err(|error| platform_error("inspect transaction journal", error))?;
    if !metadata_is_stable(&before, &after) || after.size() != bytes.len() as u64 {
        return Err(recovery_error(
            "load transaction journal",
            "transaction journal changed while it was read",
        ));
    }
    let bytes = JournalBytes::try_from_vec(bytes, max_bytes)
        .map_err(|error| recovery_error("load transaction journal", &error.to_string()))?;
    Ok(LoadedJournal {
        journal: NativeJournal {
            directory,
            file: Arc::new(file),
        },
        bytes,
    })
}

fn transaction_key_for_name(
    root: &NativeRoot,
    name: &OsStr,
) -> Result<DurableTransactionKey, TransactionFileSystemError> {
    let mut value = Vec::new();
    push_key_part(&mut value, &root.identity.value)?;
    push_key_part(&mut value, name.as_bytes())?;
    Ok(DurableTransactionKey {
        namespace: TRANSACTION_KEY_NAMESPACE.to_string(),
        value,
    })
}

fn recovery_error(operation: &'static str, reason: &str) -> TransactionFileSystemError {
    TransactionFileSystemError::Platform {
        operation,
        reason: reason.to_string(),
    }
}

#[cfg(test)]
#[path = "hashline_transaction_fs_linux_recovery_scan_tests.rs"]
mod tests;
