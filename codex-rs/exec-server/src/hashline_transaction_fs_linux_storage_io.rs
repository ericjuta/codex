use std::ffi::CStr;
use std::ffi::CString;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::num::NonZeroU64;
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use codex_hashline_transaction::DurableFileEvidence;
use codex_hashline_transaction::DurablePathKey;
use codex_hashline_transaction::ExactBytesDigest;
use codex_hashline_transaction::ExecutorFileIdentity;
use codex_hashline_transaction::FileEvidence;
use codex_hashline_transaction::FileKind;
use codex_hashline_transaction::MetadataSnapshot;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_hashline_transaction::TransactionId;

use super::NativeResolvedPath;
use super::NativeRoot;
use super::OpenKind;
use super::TRANSACTION_SIDECAR_NAME;
use super::ensure_byte_exact_directory;
use super::file_kind;
use super::identity_bytes;
use super::metadata_snapshot;
use super::open_component;
use super::storage::NativeStorage;

const MAX_TRANSACTION_ID_BYTES: usize = 128;
pub(super) const ARTIFACT_KEY_NAMESPACE: &str = "linux-hashline-artifact-v1";

pub(super) fn open_or_create_sidecar(
    root: &NativeRoot,
) -> Result<File, TransactionFileSystemError> {
    let name = OsStr::from_bytes(TRANSACTION_SIDECAR_NAME);
    match create_directory(&root.directory, name, "create transaction sidecar") {
        Ok(()) => {}
        Err(TransactionFileSystemError::Platform { reason, .. })
            if reason == io::Error::from_raw_os_error(libc::EEXIST).to_string() => {}
        Err(error) => return Err(error),
    }
    let directory = open_internal_directory(&root.directory, name)?;
    validate_internal_directory(root, &directory)?;
    Ok(directory)
}

pub(super) fn validate_internal_directory(
    root: &NativeRoot,
    directory: &File,
) -> Result<(), TransactionFileSystemError> {
    ensure_byte_exact_directory(directory)?;
    let root_metadata = root
        .directory
        .metadata()
        .map_err(|error| platform_error("inspect transaction root", error))?;
    let metadata = directory
        .metadata()
        .map_err(|error| platform_error("inspect transaction storage", error))?;
    if metadata.dev() != root_metadata.dev()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(TransactionFileSystemError::Unsupported {
            capability: "native transaction storage",
            reason: "transaction storage must be same-filesystem, owner-only, and owned by the executor user"
                .to_string(),
        });
    }
    Ok(())
}

pub(super) fn validate_transaction_id(
    transaction_id: &TransactionId,
) -> Result<(), TransactionFileSystemError> {
    let bytes = transaction_id.0.as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_TRANSACTION_ID_BYTES
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(storage_error(
            "transaction ID must be 1-128 ASCII letters, digits, hyphens, or underscores",
        ));
    }
    Ok(())
}

pub(super) fn create_directory(
    parent: &File,
    name: &OsStr,
    operation: &'static str,
) -> Result<(), TransactionFileSystemError> {
    let name = c_string(name, operation)?;
    // SAFETY: `name` is NUL-terminated and `parent` owns a live descriptor.
    let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
    if result == 0 {
        Ok(())
    } else {
        Err(platform_error(operation, io::Error::last_os_error()))
    }
}

pub(super) fn open_internal_directory(
    parent: &File,
    name: &OsStr,
) -> Result<File, TransactionFileSystemError> {
    open_component(parent, name, OpenKind::Directory)
        .map_err(|error| platform_error("open transaction storage directory", error))
}

pub(super) fn create_exclusive_file(
    parent: &File,
    name: &OsStr,
    operation: &'static str,
) -> Result<File, TransactionFileSystemError> {
    let name = c_string(name, operation)?;
    let flags = libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_CREAT | libc::O_EXCL;
    // SAFETY: `name` is NUL-terminated, `parent` owns a live descriptor, and a successful
    // descriptor is transferred exactly once into `File`.
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags, 0o600) };
    if descriptor < 0 {
        return Err(platform_error(operation, io::Error::last_os_error()));
    }
    // SAFETY: `openat` returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

pub(super) fn open_internal_file_read_write(
    parent: &File,
    name: &OsStr,
    operation: &'static str,
) -> Result<File, TransactionFileSystemError> {
    let name = c_string(name, operation)?;
    let flags = libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    // SAFETY: `name` is NUL-terminated, `parent` owns a live descriptor, and a successful
    // descriptor is transferred exactly once into `File`.
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(platform_error(operation, io::Error::last_os_error()));
    }
    // SAFETY: `openat` returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

pub(super) fn reserve_capacity(file: &File, bytes: i64) -> Result<(), TransactionFileSystemError> {
    loop {
        // SAFETY: `file` owns a writable descriptor and the offset/length are nonnegative.
        let result = unsafe { libc::posix_fallocate(file.as_raw_fd(), 0, bytes) };
        if result == 0 {
            return Ok(());
        }
        if result != libc::EINTR {
            return Err(platform_error(
                "reserve transaction capacity",
                io::Error::from_raw_os_error(result),
            ));
        }
    }
}

pub(super) fn artifact_evidence(
    storage: &NativeStorage,
    class: &[u8],
    name: &OsStr,
    file: &File,
    contents: &[u8],
) -> Result<DurableFileEvidence, TransactionFileSystemError> {
    let metadata = file
        .metadata()
        .map_err(|error| platform_error("inspect transaction artifact", error))?;
    if file_kind(&metadata) != FileKind::File
        || metadata.size() != contents.len() as u64
        || metadata.nlink() != 1
        || metadata.dev()
            != storage
                .root
                .directory
                .metadata()
                .map_err(|error| platform_error("inspect transaction root", error))?
                .dev()
    {
        return Err(storage_error("transaction artifact evidence is not stable"));
    }
    Ok(DurableFileEvidence {
        key: artifact_key(storage, class, name)?,
        evidence: FileEvidence {
            exact_digest: ExactBytesDigest::new(contents),
            identity: ExecutorFileIdentity {
                namespace: "linux-dev-ino-v1".to_string(),
                value: identity_bytes(&metadata),
            },
            metadata: metadata_snapshot(&metadata),
            link_count: NonZeroU64::MIN,
            kind: FileKind::File,
        },
    })
}

pub(super) fn apply_staged_metadata(
    file: &File,
    metadata: Option<&MetadataSnapshot>,
) -> Result<(), TransactionFileSystemError> {
    let (mode, uid, gid, modified_seconds, modified_nanoseconds) = match metadata {
        Some(metadata) => decode_metadata(metadata)?,
        None => (
            0o644,
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() },
            0,
            0,
        ),
    };
    // SAFETY: `file` owns a live descriptor and the IDs/mode are validated numeric values.
    if unsafe { libc::fchown(file.as_raw_fd(), uid, gid) } != 0 {
        return Err(platform_error(
            "set transaction artifact owner",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: `file` owns a live descriptor.
    if unsafe { libc::fchmod(file.as_raw_fd(), mode & 0o7777) } != 0 {
        return Err(platform_error(
            "set transaction artifact mode",
            io::Error::last_os_error(),
        ));
    }
    if metadata.is_some() {
        let times = [
            libc::timespec {
                tv_sec: 0,
                tv_nsec: libc::UTIME_OMIT,
            },
            libc::timespec {
                tv_sec: modified_seconds,
                tv_nsec: modified_nanoseconds,
            },
        ];
        // SAFETY: `times` contains two valid `timespec` values and `file` owns a live descriptor.
        if unsafe { libc::futimens(file.as_raw_fd(), times.as_ptr()) } != 0 {
            return Err(platform_error(
                "set transaction artifact time",
                io::Error::last_os_error(),
            ));
        }
    }
    Ok(())
}

fn decode_metadata(
    metadata: &MetadataSnapshot,
) -> Result<(u32, u32, u32, i64, i64), TransactionFileSystemError> {
    if metadata.namespace != "linux-basic-restorable-v1" {
        let namespace = &metadata.namespace;
        return Err(TransactionFileSystemError::Unsupported {
            capability: "restore Linux transaction metadata",
            reason: format!("unsupported metadata namespace `{namespace}`"),
        });
    }
    if metadata.value.len() != 28 {
        return Err(invalid_metadata_width());
    }
    let decode_u32 = |range: std::ops::Range<usize>| -> Result<u32, TransactionFileSystemError> {
        let bytes: [u8; 4] = metadata
            .value
            .get(range)
            .ok_or_else(invalid_metadata_width)?
            .try_into()
            .map_err(|_| invalid_metadata_width())?;
        Ok(u32::from_le_bytes(bytes))
    };
    let decode_i64 = |range: std::ops::Range<usize>| -> Result<i64, TransactionFileSystemError> {
        let bytes: [u8; 8] = metadata
            .value
            .get(range)
            .ok_or_else(invalid_metadata_width)?
            .try_into()
            .map_err(|_| invalid_metadata_width())?;
        Ok(i64::from_le_bytes(bytes))
    };
    let mode = decode_u32(0..4)?;
    let uid = decode_u32(4..8)?;
    let gid = decode_u32(8..12)?;
    let seconds = decode_i64(12..20)?;
    let nanoseconds = decode_i64(20..28)?;
    if !(0..1_000_000_000).contains(&nanoseconds) {
        return Err(storage_error(
            "metadata modification nanoseconds are invalid",
        ));
    }
    Ok((mode, uid, gid, seconds, nanoseconds))
}

fn invalid_metadata_width() -> TransactionFileSystemError {
    TransactionFileSystemError::Unsupported {
        capability: "restore Linux transaction metadata",
        reason: "Linux metadata snapshot has invalid width".to_string(),
    }
}

fn artifact_key(
    storage: &NativeStorage,
    class: &[u8],
    name: &OsStr,
) -> Result<DurablePathKey, TransactionFileSystemError> {
    let mut value = transaction_key_value(&storage.root, &storage.transaction_id)?;
    push_key_part(&mut value, class)?;
    push_key_part(&mut value, name.as_bytes())?;
    Ok(DurablePathKey {
        namespace: ARTIFACT_KEY_NAMESPACE.to_string(),
        value,
    })
}

pub(super) fn transaction_key_value(
    root: &NativeRoot,
    transaction_id: &TransactionId,
) -> Result<Vec<u8>, TransactionFileSystemError> {
    let mut value = Vec::new();
    push_key_part(&mut value, &root.identity.value)?;
    push_key_part(&mut value, transaction_id.0.as_bytes())?;
    Ok(value)
}

pub(super) fn push_key_part(
    value: &mut Vec<u8>,
    part: &[u8],
) -> Result<(), TransactionFileSystemError> {
    let length =
        u32::try_from(part.len()).map_err(|_| storage_error("durable key part is too large"))?;
    value.extend_from_slice(&length.to_le_bytes());
    value.extend_from_slice(part);
    Ok(())
}

pub(super) fn require_storage_root(
    storage: &NativeStorage,
    path: &NativeResolvedPath,
    operation: &'static str,
) -> Result<(), TransactionFileSystemError> {
    if storage.root.identity != path.root_identity {
        let model_path = &path.model_path;
        return Err(TransactionFileSystemError::Platform {
            operation,
            reason: format!("path `{model_path}` belongs to a different transaction root"),
        });
    }
    Ok(())
}

pub(super) fn artifact_name(prefix: &str, next: &AtomicU64) -> OsString {
    let index = next.fetch_add(1, Ordering::Relaxed);
    OsString::from(format!("{prefix}-{index:016x}"))
}

pub(super) fn rename_at(
    source_parent: &File,
    source_name: &OsStr,
    destination_parent: &File,
    destination_name: &OsStr,
) -> Result<(), TransactionFileSystemError> {
    let source_name = c_string(source_name, "rename transaction journal")?;
    let destination_name = c_string(destination_name, "rename transaction journal")?;
    // SAFETY: both names are NUL-terminated and both parents own live descriptors.
    let result = unsafe {
        libc::renameat(
            source_parent.as_raw_fd(),
            source_name.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_name.as_ptr(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(platform_error(
            "rename transaction journal",
            io::Error::last_os_error(),
        ))
    }
}

pub(super) fn unlink_at(parent: &File, name: &OsStr) -> Result<(), TransactionFileSystemError> {
    let name = c_string(name, "remove transaction artifact")?;
    // SAFETY: `name` is NUL-terminated and `parent` owns a live descriptor.
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(platform_error(
            "remove transaction artifact",
            io::Error::last_os_error(),
        ))
    }
}

pub(super) fn remove_directory_contents(
    directory: &File,
) -> Result<(), TransactionFileSystemError> {
    // SAFETY: `directory` owns a live descriptor.
    let duplicated = unsafe { libc::dup(directory.as_raw_fd()) };
    if duplicated < 0 {
        return Err(platform_error(
            "enumerate transaction artifacts",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: `duplicated` is an independently owned descriptor transferred to `fdopendir`.
    let stream = unsafe { libc::fdopendir(duplicated) };
    if stream.is_null() {
        // SAFETY: `fdopendir` did not take ownership on failure.
        unsafe { libc::close(duplicated) };
        return Err(platform_error(
            "enumerate transaction artifacts",
            io::Error::last_os_error(),
        ));
    }
    let mut names = Vec::new();
    loop {
        unsafe { *libc::__errno_location() = 0 };
        // SAFETY: `stream` remains live until the matching `closedir` below.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: `stream` is live and is closed exactly once.
            unsafe { libc::closedir(stream) };
            if error.raw_os_error() == Some(0) {
                break;
            }
            return Err(platform_error("enumerate transaction artifacts", error));
        }
        // SAFETY: `readdir` returned an initialized entry with a NUL-terminated name.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            names.push(OsString::from_vec(name.to_vec()));
        }
    }
    names.sort();
    for name in names {
        unlink_at(directory, &name)?;
    }
    directory
        .sync_all()
        .map_err(|error| platform_error("sync transaction artifact directory", error))
}

fn c_string(name: &OsStr, operation: &'static str) -> Result<CString, TransactionFileSystemError> {
    CString::new(name.as_bytes()).map_err(|_| TransactionFileSystemError::Platform {
        operation,
        reason: "internal transaction path contains NUL".to_string(),
    })
}

pub(super) fn changed_since_planning(path: &NativeResolvedPath) -> TransactionFileSystemError {
    TransactionFileSystemError::ChangedSincePlanning {
        path: path.model_path.clone(),
    }
}

pub(super) fn storage_error(reason: &str) -> TransactionFileSystemError {
    TransactionFileSystemError::Platform {
        operation: "allocate transaction storage",
        reason: reason.to_string(),
    }
}

pub(super) fn platform_error(
    operation: &'static str,
    error: io::Error,
) -> TransactionFileSystemError {
    TransactionFileSystemError::Platform {
        operation,
        reason: error.to_string(),
    }
}
