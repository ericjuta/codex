use std::ffi::OsStr;
use std::fs::File;
use std::fs::Metadata;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;

use codex_hashline_transaction::CanonicalPathKey;
use codex_hashline_transaction::FileKind;
use codex_hashline_transaction::MetadataSnapshot;
use codex_hashline_transaction::TransactionFileSystemError;

pub(super) fn canonical_path_key(
    parent: &File,
    final_component: &OsStr,
) -> Result<CanonicalPathKey, TransactionFileSystemError> {
    let metadata = parent
        .metadata()
        .map_err(|error| TransactionFileSystemError::Platform {
            operation: "resolve path",
            reason: error.to_string(),
        })?;
    let mut value = identity_bytes(&metadata);
    value.push(0);
    value.extend_from_slice(final_component.as_bytes());
    Ok(CanonicalPathKey {
        namespace: "linux-parent-entry-v1".to_string(),
        value,
    })
}

pub(super) fn identity_bytes(metadata: &Metadata) -> Vec<u8> {
    let mut value = Vec::with_capacity(16);
    value.extend_from_slice(&metadata.dev().to_le_bytes());
    value.extend_from_slice(&metadata.ino().to_le_bytes());
    value
}

pub(super) fn metadata_snapshot(metadata: &Metadata) -> MetadataSnapshot {
    let mut value = Vec::with_capacity(32);
    value.extend_from_slice(&metadata.mode().to_le_bytes());
    value.extend_from_slice(&metadata.uid().to_le_bytes());
    value.extend_from_slice(&metadata.gid().to_le_bytes());
    value.extend_from_slice(&metadata.mtime().to_le_bytes());
    value.extend_from_slice(&metadata.mtime_nsec().to_le_bytes());
    MetadataSnapshot::new("linux-basic-restorable-v1".to_string(), value)
}

pub(super) fn metadata_is_stable(before: &Metadata, after: &Metadata) -> bool {
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.mode() == after.mode()
        && before.nlink() == after.nlink()
        && before.uid() == after.uid()
        && before.gid() == after.gid()
        && before.size() == after.size()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

pub(super) fn file_kind(metadata: &Metadata) -> FileKind {
    let kind = metadata.file_type();
    if kind.is_file() {
        FileKind::File
    } else if kind.is_dir() {
        FileKind::Directory
    } else if kind.is_symlink() {
        FileKind::SymbolicLink
    } else {
        FileKind::Other
    }
}
