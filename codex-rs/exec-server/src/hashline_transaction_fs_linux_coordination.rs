use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;

use codex_hashline_transaction::CanonicalPathKey;
use codex_hashline_transaction::ExecutorRootIdentity;
use codex_hashline_transaction::ObservationLimit;
use codex_hashline_transaction::ObservedPath;
use codex_hashline_transaction::PlanningFileSystem;
use codex_hashline_transaction::TransactionCoordination;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_utils_path_uri::PathUri;

use super::NativePlanningFileSystem;
use super::NativeResolvedPath;
use super::NativeRoot;
use super::run_blocking;
use super::verify_directory_chain;
use crate::hashline_transaction_fs::NativeTransactionFileSystem;

#[derive(Debug)]
pub struct NativeLease {
    root: NativeRoot,
    root_identity: ExecutorRootIdentity,
    covered_paths: BTreeSet<CanonicalPathKey>,
    _locked_directories: Vec<LockedDirectory>,
}

impl NativeLease {
    pub(super) fn root(&self) -> &NativeRoot {
        &self.root
    }
    pub(super) fn require_path(
        &self,
        path: &NativeResolvedPath,
        operation: &'static str,
    ) -> Result<(), TransactionFileSystemError> {
        if self.root_identity != path.root_identity || !self.covered_paths.contains(&path.key) {
            return Err(TransactionFileSystemError::Platform {
                operation,
                reason: format!(
                    "path `{}` is not covered by the retained transaction lease",
                    path.model_path
                ),
            });
        }
        Ok(())
    }
}

#[derive(Debug)]
struct LockedDirectory {
    _handle: File,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug)]
struct LockTarget {
    identity: DirectoryIdentity,
    parent: Arc<File>,
}

impl PlanningFileSystem for NativeTransactionFileSystem {
    type Root = NativeRoot;
    type ResolvedPath = NativeResolvedPath;

    async fn open_root(&self, root: &PathUri) -> Result<Self::Root, TransactionFileSystemError> {
        self.ensure_configured_root(root)?;
        NativePlanningFileSystem.open_root(root).await
    }

    async fn resolve(
        &self,
        root: &Self::Root,
        model_path: &str,
    ) -> Result<Self::ResolvedPath, TransactionFileSystemError> {
        NativePlanningFileSystem.resolve(root, model_path).await
    }

    async fn observe(
        &self,
        path: &Self::ResolvedPath,
        limit: ObservationLimit,
    ) -> Result<ObservedPath, TransactionFileSystemError> {
        NativePlanningFileSystem.observe(path, limit).await
    }

    fn root_identity(
        &self,
        root: &Self::Root,
    ) -> Result<ExecutorRootIdentity, TransactionFileSystemError> {
        NativePlanningFileSystem.root_identity(root)
    }

    fn canonical_path_key(
        &self,
        path: &Self::ResolvedPath,
    ) -> Result<CanonicalPathKey, TransactionFileSystemError> {
        NativePlanningFileSystem.canonical_path_key(path)
    }
}

impl TransactionCoordination for NativeTransactionFileSystem {
    type Lease = NativeLease;

    async fn lock_paths(
        &self,
        root: &Self::Root,
        paths: &[Self::ResolvedPath],
    ) -> Result<Self::Lease, TransactionFileSystemError> {
        let root = root.clone();
        let paths = paths.to_vec();
        run_blocking("lock transaction paths", move || {
            acquire_path_locks(&root, &paths)
        })
        .await
    }

    async fn reobserve_locked(
        &self,
        lease: &Self::Lease,
        path: &Self::ResolvedPath,
        limit: ObservationLimit,
    ) -> Result<ObservedPath, TransactionFileSystemError> {
        if lease.root_identity != path.root_identity || !lease.covered_paths.contains(&path.key) {
            return Err(TransactionFileSystemError::Platform {
                operation: "reobserve locked path",
                reason: format!(
                    "path `{}` is not covered by the retained transaction lease",
                    path.model_path
                ),
            });
        }
        self.observe(path, limit).await
    }
}

fn acquire_path_locks(
    root: &NativeRoot,
    paths: &[NativeResolvedPath],
) -> Result<NativeLease, TransactionFileSystemError> {
    let (targets, covered_paths) = collect_lock_targets(root, paths)?;
    let mut locked_directories = Vec::with_capacity(targets.len());
    for target in targets {
        let handle = open_independent_directory_handle(&target.parent)?;
        let identity = directory_identity(&handle, "inspect transaction lock")?;
        if identity != target.identity {
            return Err(TransactionFileSystemError::Platform {
                operation: "lock transaction paths",
                reason: "retained parent directory identity changed before locking".to_string(),
            });
        }
        lock_exclusive(&handle)?;
        locked_directories.push(LockedDirectory { _handle: handle });
    }
    for path in paths {
        verify_directory_chain(path)?;
    }
    Ok(NativeLease {
        root: root.clone(),
        root_identity: root.identity.clone(),
        covered_paths,
        _locked_directories: locked_directories,
    })
}

fn collect_lock_targets(
    root: &NativeRoot,
    paths: &[NativeResolvedPath],
) -> Result<(Vec<LockTarget>, BTreeSet<CanonicalPathKey>), TransactionFileSystemError> {
    let mut targets = BTreeMap::new();
    let mut covered_paths = BTreeSet::new();
    for path in paths {
        if path.root_identity != root.identity {
            return Err(TransactionFileSystemError::Platform {
                operation: "lock transaction paths",
                reason: format!(
                    "path `{}` belongs to a different transaction root",
                    path.model_path
                ),
            });
        }
        verify_directory_chain(path)?;
        let identity = directory_identity(&path.parent, "inspect transaction lock")?;
        targets
            .entry(identity)
            .or_insert_with(|| Arc::clone(&path.parent));
        covered_paths.insert(path.key.clone());
    }
    let targets = targets
        .into_iter()
        .map(|(identity, parent)| LockTarget { identity, parent })
        .collect();
    Ok((targets, covered_paths))
}

fn open_independent_directory_handle(parent: &File) -> Result<File, TransactionFileSystemError> {
    let current = c".";
    let flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW;
    // SAFETY: `current` is NUL-terminated, `parent` owns a live descriptor, and a successful
    // descriptor is transferred exactly once into `File`.
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), current.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(platform_error(
            "open transaction lock",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: `openat` returned a new owned descriptor and not a duplicate of `parent`'s open
    // file description, so independent transactions contend even inside one process.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn lock_exclusive(handle: &File) -> Result<(), TransactionFileSystemError> {
    loop {
        // SAFETY: `handle` owns a live descriptor. The lock is released when the lease drops the
        // retained file handle.
        let result = unsafe { libc::flock(handle.as_raw_fd(), libc::LOCK_EX) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(platform_error("lock transaction paths", error));
        }
    }
}

fn directory_identity(
    directory: &File,
    operation: &'static str,
) -> Result<DirectoryIdentity, TransactionFileSystemError> {
    let metadata = directory
        .metadata()
        .map_err(|error| platform_error(operation, error))?;
    Ok(DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn platform_error(operation: &'static str, error: io::Error) -> TransactionFileSystemError {
    TransactionFileSystemError::Platform {
        operation,
        reason: error.to_string(),
    }
}

#[cfg(test)]
#[path = "hashline_transaction_fs_linux_coordination_tests.rs"]
mod tests;
