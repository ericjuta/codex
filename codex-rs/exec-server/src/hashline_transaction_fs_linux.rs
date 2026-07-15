use std::ffi::CString;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::num::NonZeroU64;
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;
use std::os::unix::fs::MetadataExt;
use std::path::Component;
use std::path::Path;
use std::sync::Arc;

use codex_hashline_transaction::CanonicalPathKey;
use codex_hashline_transaction::ExecutorFileIdentity;
use codex_hashline_transaction::ExecutorRootIdentity;
use codex_hashline_transaction::FileKind;
use codex_hashline_transaction::ObservationLimit;
use codex_hashline_transaction::ObservedFile;
use codex_hashline_transaction::ObservedPath;
use codex_hashline_transaction::PlanningFileSystem;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;

use super::NativePlanningFileSystem;

#[path = "hashline_transaction_fs_linux_coordination.rs"]
mod coordination;
#[path = "hashline_transaction_fs_linux_evidence.rs"]
mod evidence;
#[path = "hashline_transaction_fs_linux_mutation.rs"]
mod mutation;
#[path = "hashline_transaction_fs_linux_recovery.rs"]
mod recovery;
#[path = "hashline_transaction_fs_linux_recovery_io.rs"]
mod recovery_io;
#[path = "hashline_transaction_fs_linux_recovery_scan.rs"]
mod recovery_scan;
#[path = "hashline_transaction_fs_linux_semantics.rs"]
mod semantics;
#[path = "hashline_transaction_fs_linux_storage.rs"]
mod storage;
#[path = "hashline_transaction_fs_linux_storage_io.rs"]
mod storage_io;

use evidence::canonical_path_key;
use evidence::file_kind;
use evidence::identity_bytes;
use evidence::metadata_is_stable;
use evidence::metadata_snapshot;
use semantics::ensure_byte_exact_directory;

const READ_BLOCK_BYTES: usize = 64 * 1024;
const TRANSACTION_SIDECAR_NAME: &[u8] = b".codex-hashline-transactions";

#[derive(Clone, Debug)]
pub struct NativeRoot {
    directory: Arc<File>,
    absolute: AbsolutePathBuf,
    identity: ExecutorRootIdentity,
}

#[derive(Clone, Debug)]
pub struct NativeResolvedPath {
    parent: Arc<File>,
    directory_chain: Vec<DirectoryEdge>,
    final_component: OsString,
    object: Option<Arc<File>>,
    root_identity: ExecutorRootIdentity,
    key: CanonicalPathKey,
    model_path: String,
}

#[derive(Clone, Debug)]
struct DirectoryEdge {
    parent: Arc<File>,
    child: Arc<File>,
    component: OsString,
}

impl PartialEq for NativeResolvedPath {
    fn eq(&self, other: &Self) -> bool {
        self.root_identity == other.root_identity && self.key == other.key
    }
}

impl Eq for NativeResolvedPath {}

impl PlanningFileSystem for NativePlanningFileSystem {
    type Root = NativeRoot;
    type ResolvedPath = NativeResolvedPath;

    async fn open_root(&self, root: &PathUri) -> Result<Self::Root, TransactionFileSystemError> {
        let root = root.clone();
        run_blocking("open root", move || open_root(&root)).await
    }

    async fn resolve(
        &self,
        root: &Self::Root,
        model_path: &str,
    ) -> Result<Self::ResolvedPath, TransactionFileSystemError> {
        let root = root.clone();
        let model_path = model_path.to_string();
        run_blocking("resolve path", move || resolve(&root, &model_path)).await
    }

    async fn observe(
        &self,
        path: &Self::ResolvedPath,
        limit: ObservationLimit,
    ) -> Result<ObservedPath, TransactionFileSystemError> {
        let path = path.clone();
        run_blocking("observe path", move || observe(&path, limit)).await
    }

    fn root_identity(
        &self,
        root: &Self::Root,
    ) -> Result<ExecutorRootIdentity, TransactionFileSystemError> {
        Ok(root.identity.clone())
    }

    fn canonical_path_key(
        &self,
        path: &Self::ResolvedPath,
    ) -> Result<CanonicalPathKey, TransactionFileSystemError> {
        Ok(path.key.clone())
    }
}

async fn run_blocking<T, F>(
    operation: &'static str,
    task: F,
) -> Result<T, TransactionFileSystemError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, TransactionFileSystemError> + Send + 'static,
{
    tokio::task::spawn_blocking(task).await.map_err(|error| {
        TransactionFileSystemError::Platform {
            operation,
            reason: format!("blocking filesystem task failed: {error}"),
        }
    })?
}

fn open_root(root: &PathUri) -> Result<NativeRoot, TransactionFileSystemError> {
    let absolute = root
        .to_abs_path()
        .map_err(|error| TransactionFileSystemError::InvalidRoot {
            root: root.clone(),
            reason: error.to_string(),
        })?;
    let mut directory = File::open("/").map_err(|error| platform_error("open root", error))?;
    for component in absolute.as_path().components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => {
                directory =
                    open_component(&directory, name, OpenKind::Directory).map_err(|error| {
                        classify_open_error(&directory, name, &root.to_string(), "open root", error)
                    })?;
            }
            Component::ParentDir | Component::Prefix(_) => {
                return Err(TransactionFileSystemError::InvalidRoot {
                    root: root.clone(),
                    reason: "root contains a non-native or parent component".to_string(),
                });
            }
        }
    }
    let metadata = directory
        .metadata()
        .map_err(|error| platform_error("inspect root", error))?;
    ensure_byte_exact_directory(&directory)?;
    Ok(NativeRoot {
        directory: Arc::new(directory),
        absolute,
        identity: ExecutorRootIdentity {
            namespace: "linux-dev-ino-v1".to_string(),
            value: identity_bytes(&metadata),
        },
    })
}

fn resolve(
    root: &NativeRoot,
    model_path: &str,
) -> Result<NativeResolvedPath, TransactionFileSystemError> {
    let components = relative_components(root, model_path)?;
    resolve_components(root, &components, model_path)
}

fn resolve_components(
    root: &NativeRoot,
    components: &[OsString],
    model_path: &str,
) -> Result<NativeResolvedPath, TransactionFileSystemError> {
    let Some((final_component, parents)) = components.split_last() else {
        return Err(invalid_model_path(
            model_path,
            "path resolves to the selected root",
        ));
    };
    let mut parent = Arc::clone(&root.directory);
    let mut directory_chain = Vec::new();
    for component in parents {
        let directory = Arc::new(
            open_component(&parent, component, OpenKind::Directory).map_err(|error| {
                classify_open_error(&parent, component, model_path, "resolve path", error)
            })?,
        );
        ensure_byte_exact_directory(&directory)?;
        directory_chain.push(DirectoryEdge {
            parent: Arc::clone(&parent),
            child: Arc::clone(&directory),
            component: component.clone(),
        });
        parent = directory;
    }
    let object = match open_component(&parent, final_component, OpenKind::Any) {
        Ok(file) => Some(Arc::new(file)),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                && !entry_is_symlink(&parent, final_component) =>
        {
            None
        }
        Err(error) => {
            return Err(classify_open_error(
                &parent,
                final_component,
                model_path,
                "resolve path",
                error,
            ));
        }
    };
    let key = canonical_path_key(&parent, final_component)?;
    Ok(NativeResolvedPath {
        parent,
        directory_chain,
        final_component: final_component.clone(),
        object,
        root_identity: root.identity.clone(),
        key,
        model_path: model_path.to_string(),
    })
}

fn relative_components(
    root: &NativeRoot,
    model_path: &str,
) -> Result<Vec<OsString>, TransactionFileSystemError> {
    if model_path.is_empty() {
        return Err(invalid_model_path(model_path, "path is empty"));
    }
    let path = Path::new(model_path);
    let relative = if path.is_absolute() {
        path.strip_prefix(root.absolute.as_path()).map_err(|_| {
            invalid_model_path(model_path, "absolute path is outside the selected root")
        })?
    } else {
        path
    };
    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => {
                if name.as_bytes().contains(&0) {
                    return Err(invalid_model_path(model_path, "path contains NUL"));
                }
                components.push(name.to_os_string());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(invalid_model_path(
                    model_path,
                    "parent traversal is not allowed",
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(invalid_model_path(
                    model_path,
                    "path is not executor-native",
                ));
            }
        }
    }
    if components.is_empty() {
        return Err(invalid_model_path(
            model_path,
            "path resolves to the selected root",
        ));
    }
    if components
        .first()
        .is_some_and(|component| component.as_bytes() == TRANSACTION_SIDECAR_NAME)
    {
        return Err(invalid_model_path(
            model_path,
            "path uses the reserved Hashline transaction sidecar name",
        ));
    }
    Ok(components)
}

fn observe(
    path: &NativeResolvedPath,
    limit: ObservationLimit,
) -> Result<ObservedPath, TransactionFileSystemError> {
    verify_directory_chain(path)?;
    let Some(file) = &path.object else {
        return verify_absent(path);
    };
    let before = file
        .metadata()
        .map_err(|error| platform_error("observe path", error))?;
    let kind = file_kind(&before);
    let contents = if kind == FileKind::File {
        read_bounded(file, before.size(), limit)?
    } else {
        Vec::new()
    };
    let after = file
        .metadata()
        .map_err(|error| platform_error("observe path", error))?;
    if !metadata_is_stable(&before, &after)
        || (kind == FileKind::File && contents.len() as u64 != after.size())
    {
        return Err(changed_during_planning(&path.model_path));
    }
    let rebound = open_component(&path.parent, &path.final_component, OpenKind::Any)
        .map_err(|_| changed_during_planning(&path.model_path))?;
    let rebound_metadata = rebound
        .metadata()
        .map_err(|_| changed_during_planning(&path.model_path))?;
    if identity_bytes(&after) != identity_bytes(&rebound_metadata) {
        return Err(changed_during_planning(&path.model_path));
    }
    verify_directory_chain(path)?;
    let link_count =
        NonZeroU64::new(after.nlink()).ok_or_else(|| TransactionFileSystemError::Platform {
            operation: "observe path",
            reason: format!("path `{}` reported a zero link count", path.model_path),
        })?;
    Ok(ObservedPath::Present(ObservedFile::new(
        contents,
        ExecutorFileIdentity {
            namespace: "linux-dev-ino-v1".to_string(),
            value: identity_bytes(&after),
        },
        metadata_snapshot(&after),
        link_count,
        kind,
    )))
}

fn verify_absent(path: &NativeResolvedPath) -> Result<ObservedPath, TransactionFileSystemError> {
    verify_directory_chain(path)?;
    let observed = match open_component(&path.parent, &path.final_component, OpenKind::Any) {
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                && !entry_is_symlink(&path.parent, &path.final_component) =>
        {
            Ok(ObservedPath::Absent)
        }
        Ok(_) | Err(_) => Err(changed_during_planning(&path.model_path)),
    };
    verify_directory_chain(path)?;
    observed
}

fn verify_directory_chain(path: &NativeResolvedPath) -> Result<(), TransactionFileSystemError> {
    for edge in &path.directory_chain {
        ensure_byte_exact_directory(&edge.parent)?;
        let reopened = open_component(&edge.parent, &edge.component, OpenKind::Directory)
            .map_err(|_| changed_during_planning(&path.model_path))?;
        let retained_metadata = edge
            .child
            .metadata()
            .map_err(|_| changed_during_planning(&path.model_path))?;
        let reopened_metadata = reopened
            .metadata()
            .map_err(|_| changed_during_planning(&path.model_path))?;
        if identity_bytes(&retained_metadata) != identity_bytes(&reopened_metadata) {
            return Err(changed_during_planning(&path.model_path));
        }
    }
    ensure_byte_exact_directory(&path.parent)
}

fn read_bounded(
    file: &File,
    observed_size: u64,
    limit: ObservationLimit,
) -> Result<Vec<u8>, TransactionFileSystemError> {
    if observed_size > limit.max_bytes {
        return Err(observation_limit_error(observed_size, limit.max_bytes));
    }
    let capacity = observed_size.min(READ_BLOCK_BYTES as u64) as usize;
    let mut contents = Vec::with_capacity(capacity);
    let mut block = [0; READ_BLOCK_BYTES];
    let mut offset = 0_u64;
    loop {
        let remaining = limit.max_bytes.saturating_sub(offset);
        let requested = if remaining == 0 {
            1
        } else {
            let remaining_with_sentinel =
                usize::try_from(remaining.saturating_add(1)).unwrap_or(usize::MAX);
            READ_BLOCK_BYTES.min(remaining_with_sentinel)
        };
        let read = loop {
            match file.read_at(&mut block[..requested], offset) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                result => break result.map_err(|error| platform_error("observe path", error))?,
            }
        };
        if read == 0 {
            break;
        }
        let new_size = offset.saturating_add(read as u64);
        if new_size > limit.max_bytes {
            return Err(observation_limit_error(new_size, limit.max_bytes));
        }
        contents.extend_from_slice(&block[..read]);
        offset = new_size;
    }
    Ok(contents)
}

#[derive(Clone, Copy)]
enum OpenKind {
    Any,
    Directory,
}

fn open_component(parent: &File, name: &OsStr, kind: OpenKind) -> io::Result<File> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let kind_flags = match kind {
        OpenKind::Any => libc::O_NONBLOCK,
        OpenKind::Directory => libc::O_DIRECTORY,
    };
    let flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | kind_flags;
    // SAFETY: `name` is NUL-terminated, `parent` owns a live descriptor, and a
    // successful descriptor is transferred exactly once into `File`.
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `openat` returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn entry_is_symlink(parent: &File, name: &OsStr) -> bool {
    let Ok(name) = CString::new(name.as_bytes()) else {
        return false;
    };
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `stat` points to writable storage and `name` is NUL-terminated.
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return false;
    }
    // SAFETY: successful `fstatat` initialized `stat`.
    let stat = unsafe { stat.assume_init() };
    stat.st_mode & libc::S_IFMT == libc::S_IFLNK
}

fn classify_open_error(
    parent: &File,
    name: &OsStr,
    path: &str,
    operation: &'static str,
    error: io::Error,
) -> TransactionFileSystemError {
    if entry_is_symlink(parent, name) {
        TransactionFileSystemError::SymbolicLink {
            path: path.to_string(),
        }
    } else {
        platform_error(operation, error)
    }
}

fn invalid_model_path(path: &str, reason: &str) -> TransactionFileSystemError {
    TransactionFileSystemError::InvalidModelPath {
        path: path.to_string(),
        reason: reason.to_string(),
    }
}

fn platform_error(operation: &'static str, error: io::Error) -> TransactionFileSystemError {
    TransactionFileSystemError::Platform {
        operation,
        reason: error.to_string(),
    }
}

fn changed_during_planning(path: &str) -> TransactionFileSystemError {
    TransactionFileSystemError::ChangedSincePlanning {
        path: path.to_string(),
    }
}

fn observation_limit_error(observed: u64, limit: u64) -> TransactionFileSystemError {
    TransactionFileSystemError::Platform {
        operation: "observe path",
        reason: format!("file requires at least {observed} bytes, exceeding limit {limit}"),
    }
}
