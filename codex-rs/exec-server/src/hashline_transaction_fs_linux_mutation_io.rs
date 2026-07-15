use std::ffi::CString;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::num::NonZeroU64;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;

use codex_hashline_transaction::DurableFileEvidence;
use codex_hashline_transaction::ExecutorFileIdentity;
use codex_hashline_transaction::FileEvidence;
use codex_hashline_transaction::FileKind;
use codex_hashline_transaction::ObservationLimit;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_hashline_transaction::TransactionStorage;

use crate::hashline_transaction_fs::NativeTransactionFileSystem;
use crate::hashline_transaction_fs::platform::NativeResolvedPath;
use crate::hashline_transaction_fs::platform::OpenKind;
use crate::hashline_transaction_fs::platform::entry_is_symlink;
use crate::hashline_transaction_fs::platform::file_kind;
use crate::hashline_transaction_fs::platform::identity_bytes;
use crate::hashline_transaction_fs::platform::metadata_is_stable;
use crate::hashline_transaction_fs::platform::metadata_snapshot;
use crate::hashline_transaction_fs::platform::open_component;
use crate::hashline_transaction_fs::platform::read_bounded;
use crate::hashline_transaction_fs::platform::storage::NativeBackup;
use crate::hashline_transaction_fs::platform::storage::NativeJournal;
use crate::hashline_transaction_fs::platform::storage::NativeStagedFile;
use crate::hashline_transaction_fs::platform::storage_io::platform_error;
use crate::hashline_transaction_fs::platform::verify_directory_chain;

#[derive(Clone)]
pub(super) struct Artifact {
    parent: Arc<File>,
    name: OsString,
    file: Arc<File>,
    evidence: FileEvidence,
}

impl Artifact {
    pub(super) fn validate(&self) -> Result<FileEvidence, TransactionFileSystemError> {
        let retained = file_evidence(&self.file)?;
        if !same_core(&retained, &self.evidence) || !matches!(retained.link_count.get(), 1 | 2) {
            return Err(changed("transaction artifact"));
        }
        let reopened = entry_state(&self.parent, &self.name, "transaction artifact")?;
        let EntryState::Present(reopened) = reopened else {
            return Err(changed("transaction artifact"));
        };
        if reopened != retained {
            return Err(changed("transaction artifact"));
        }
        Ok(retained)
    }

    pub(super) fn validate_before(
        &self,
        expected: &FileEvidence,
    ) -> Result<(), TransactionFileSystemError> {
        let evidence = self.validate()?;
        if evidence.exact_digest != expected.exact_digest
            || evidence.metadata != expected.metadata
            || evidence.kind != expected.kind
            || self.evidence.link_count != NonZeroU64::MIN
            || expected.link_count != NonZeroU64::MIN
        {
            return Err(changed("transaction backup"));
        }
        Ok(())
    }

    pub(super) fn matches_linked(&self, state: &EntryState) -> bool {
        let Ok(artifact) = self.validate() else {
            return false;
        };
        matches!(
            state,
            EntryState::Present(candidate)
                if same_core(candidate, &artifact)
                    && candidate.link_count.get() == 2
                    && artifact.link_count.get() == 2
        )
    }

    pub(super) fn identity(&self) -> &[u8] {
        &self.evidence.identity.value
    }
}

pub(super) enum Guard {
    Evidence(FileEvidence),
    Artifact(Artifact),
}

impl Guard {
    pub(super) fn matches(&self, state: &EntryState) -> bool {
        match self {
            Self::Evidence(evidence) => matches_evidence(state, evidence),
            Self::Artifact(artifact) => artifact.matches_linked(state),
        }
    }

    pub(super) fn identity(&self) -> &[u8] {
        match self {
            Self::Evidence(evidence) => &evidence.identity.value,
            Self::Artifact(artifact) => artifact.identity(),
        }
    }
}

#[derive(Debug)]
pub(super) enum EntryState {
    Absent,
    Present(FileEvidence),
}

pub(super) fn staged_artifact(
    file_system: &NativeTransactionFileSystem,
    staged: &NativeStagedFile,
) -> Result<Artifact, TransactionFileSystemError> {
    artifact(
        &staged.parent,
        &staged.name,
        &staged.file,
        file_system.staged_file_evidence(staged)?,
    )
}

pub(super) fn backup_artifact(
    file_system: &NativeTransactionFileSystem,
    backup: &NativeBackup,
) -> Result<Artifact, TransactionFileSystemError> {
    artifact(
        &backup.parent,
        &backup.name,
        &backup.file,
        file_system.backup_evidence(backup)?,
    )
}

fn artifact(
    parent: &Arc<File>,
    name: &OsString,
    file: &Arc<File>,
    durable: DurableFileEvidence,
) -> Result<Artifact, TransactionFileSystemError> {
    if durable.evidence.kind != FileKind::File || durable.evidence.link_count != NonZeroU64::MIN {
        return Err(changed("transaction artifact"));
    }
    Ok(Artifact {
        parent: Arc::clone(parent),
        name: name.clone(),
        file: Arc::clone(file),
        evidence: durable.evidence,
    })
}

pub(super) fn require_journal(journal: &NativeJournal) -> Result<(), TransactionFileSystemError> {
    for file in [&journal.directory, &journal.file] {
        file.metadata()
            .map_err(|error| platform_error("inspect transaction journal handle", error))?;
    }
    Ok(())
}

pub(super) fn path_state(
    path: &NativeResolvedPath,
) -> Result<EntryState, TransactionFileSystemError> {
    verify_directory_chain(path)?;
    let state = entry_state(&path.parent, &path.final_component, &path.model_path)?;
    verify_directory_chain(path)?;
    Ok(state)
}

pub(super) fn entry_state(
    parent: &File,
    name: &OsStr,
    label: &str,
) -> Result<EntryState, TransactionFileSystemError> {
    match open_component(parent, name, OpenKind::Any) {
        Ok(file) => Ok(EntryState::Present(file_evidence(&file)?)),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound && !entry_is_symlink(parent, name) =>
        {
            Ok(EntryState::Absent)
        }
        Err(_error) if entry_is_symlink(parent, name) => {
            Err(TransactionFileSystemError::SymbolicLink {
                path: label.to_string(),
            })
        }
        Err(error) => Err(platform_error("inspect transaction entry", error)),
    }
}

fn file_evidence(file: &File) -> Result<FileEvidence, TransactionFileSystemError> {
    let before = file
        .metadata()
        .map_err(|error| platform_error("inspect transaction entry", error))?;
    let kind = file_kind(&before);
    let contents = if kind == FileKind::File {
        read_bounded(
            file,
            before.size(),
            ObservationLimit {
                max_bytes: before.size(),
            },
        )?
    } else {
        Vec::new()
    };
    let after = file
        .metadata()
        .map_err(|error| platform_error("reinspect transaction entry", error))?;
    if !metadata_is_stable(&before, &after)
        || (kind == FileKind::File && contents.len() as u64 != after.size())
    {
        return Err(changed("transaction entry"));
    }
    Ok(FileEvidence {
        exact_digest: codex_hashline_transaction::ExactBytesDigest::new(&contents),
        identity: ExecutorFileIdentity {
            namespace: "linux-dev-ino-v1".to_string(),
            value: identity_bytes(&after),
        },
        metadata: metadata_snapshot(&after),
        link_count: NonZeroU64::new(after.nlink())
            .ok_or_else(|| changed("transaction entry link topology"))?,
        kind,
    })
}

fn same_core(left: &FileEvidence, right: &FileEvidence) -> bool {
    left.exact_digest == right.exact_digest
        && left.identity == right.identity
        && left.metadata == right.metadata
        && left.kind == right.kind
}

pub(super) fn matches_evidence(state: &EntryState, expected: &FileEvidence) -> bool {
    matches!(state, EntryState::Present(actual) if actual == expected)
}

pub(super) fn link_artifact(
    artifact: &Artifact,
    destination_parent: &File,
    destination_name: &OsStr,
) -> Result<(), TransactionFileSystemError> {
    let source_name = c_string(&artifact.name, "link transaction artifact")?;
    let destination_name = c_string(destination_name, "link transaction artifact")?;
    // SAFETY: both names are NUL-terminated and both descriptors remain live for the call.
    let result = unsafe {
        libc::linkat(
            artifact.parent.as_raw_fd(),
            source_name.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_name.as_ptr(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(platform_error(
            "link transaction artifact",
            io::Error::last_os_error(),
        ))
    }
}

pub(super) fn rename_with_flags(
    source_parent: &File,
    source_name: &OsStr,
    destination_parent: &File,
    destination_name: &OsStr,
    flags: libc::c_uint,
    operation: &'static str,
) -> Result<(), TransactionFileSystemError> {
    let source_name = c_string(source_name, operation)?;
    let destination_name = c_string(destination_name, operation)?;
    // SAFETY: both names are NUL-terminated and both descriptors remain live for the call.
    let result = unsafe {
        libc::renameat2(
            source_parent.as_raw_fd(),
            source_name.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_name.as_ptr(),
            flags,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOSYS) {
            Err(TransactionFileSystemError::Unsupported {
                capability: "guarded Linux transaction mutation",
                reason: "renameat2 is unavailable".to_string(),
            })
        } else {
            Err(platform_error(operation, error))
        }
    }
}

pub(super) fn unlink_entry(
    parent: &File,
    name: &OsStr,
    operation: &'static str,
) -> Result<(), TransactionFileSystemError> {
    let name = c_string(name, operation)?;
    // SAFETY: the name is NUL-terminated and the descriptor remains live for the call.
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(platform_error(operation, io::Error::last_os_error()))
    }
}

pub(super) fn temporary_name(class: &str, role: &str, identity: &[u8]) -> OsString {
    let mut value = format!(".codex-hashline-{class}-{role}-");
    for byte in identity {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    OsString::from(value)
}

pub(super) fn classify_entry_error(
    path: &NativeResolvedPath,
    error: io::Error,
) -> TransactionFileSystemError {
    if entry_is_symlink(&path.parent, &path.final_component) {
        TransactionFileSystemError::SymbolicLink {
            path: path.model_path.clone(),
        }
    } else {
        platform_error("open transaction metadata target", error)
    }
}

pub(super) fn changed(path: &str) -> TransactionFileSystemError {
    TransactionFileSystemError::ChangedSincePlanning {
        path: path.to_string(),
    }
}

fn c_string(value: &OsStr, operation: &'static str) -> Result<CString, TransactionFileSystemError> {
    CString::new(value.as_bytes()).map_err(|_| TransactionFileSystemError::Platform {
        operation,
        reason: "path contains NUL".to_string(),
    })
}
