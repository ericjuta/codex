use std::ffi::OsString;
use std::fs::File;
use std::num::NonZeroU64;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::Arc;

use codex_hashline_transaction::DurableFileEvidence;
use codex_hashline_transaction::DurablePathKey;
use codex_hashline_transaction::DurableTransactionKey;
use codex_hashline_transaction::ExactBytesDigest;
use codex_hashline_transaction::ExecutorFileIdentity;
use codex_hashline_transaction::FileEvidence;
use codex_hashline_transaction::FileKind;
use codex_hashline_transaction::ObservationLimit;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_hashline_transaction::TransactionId;
use codex_hashline_transaction::TransactionLimits;

use super::NativeResolvedPath;
use super::NativeRoot;
use super::OpenKind;
use super::TRANSACTION_SIDECAR_NAME;
use super::file_kind;
use super::identity_bytes;
use super::metadata_is_stable;
use super::metadata_snapshot;
use super::open_component;
use super::read_bounded;
use super::resolve_components;
use super::storage::NativeStorage;
use super::storage_io::ARTIFACT_KEY_NAMESPACE;

const PATH_KEY_NAMESPACE: &str = "linux-hashline-path-v1";
const TRANSACTION_KEY_NAMESPACE: &str = "linux-hashline-transaction-v1";
const MAX_DURABLE_PATH_COMPONENTS: usize = 256;

pub(super) enum ArtifactClass {
    Staged,
    Backup,
}

impl ArtifactClass {
    fn bytes(&self) -> &'static [u8] {
        match self {
            Self::Staged => b"staged",
            Self::Backup => b"backup",
        }
    }

    fn directory(&self, storage: &NativeStorage) -> Arc<File> {
        match self {
            Self::Staged => Arc::clone(&storage.staged_directory),
            Self::Backup => Arc::clone(&storage.backup_directory),
        }
    }
}

pub(super) struct ReopenedArtifact {
    pub(super) parent: Arc<File>,
    pub(super) name: OsString,
    pub(super) file: Arc<File>,
}

pub(super) fn reopen_path_from_key(
    root: &NativeRoot,
    key: &DurablePathKey,
) -> Result<NativeResolvedPath, TransactionFileSystemError> {
    if key.namespace != PATH_KEY_NAMESPACE {
        return Err(TransactionFileSystemError::Unsupported {
            capability: "native transaction recovery path",
            reason: "durable path key namespace is not supported".to_string(),
        });
    }
    let (root_identity, mut remainder) = take_key_part(&key.value)?;
    if root_identity != root.identity.value {
        return Err(TransactionFileSystemError::ChangedSincePlanning {
            path: root.absolute.as_path().display().to_string(),
        });
    }
    let mut components = Vec::new();
    while !remainder.is_empty() {
        if components.len() == MAX_DURABLE_PATH_COMPONENTS {
            return Err(recovery_error(
                "decode transaction recovery path",
                "durable path has too many components",
            ));
        }
        let (component, next) = take_key_part(remainder)?;
        validate_component(component, components.is_empty())?;
        components.push(OsString::from_vec(component.to_vec()));
        remainder = next;
    }
    if components.is_empty() {
        return Err(recovery_error(
            "decode transaction recovery path",
            "durable path has no leaf component",
        ));
    }
    let mut display_path = PathBuf::new();
    for component in &components {
        display_path.push(component);
    }
    resolve_components(root, &components, &display_path.to_string_lossy())
}

pub(super) fn reopen_artifact(
    storage: &NativeStorage,
    evidence: &DurableFileEvidence,
    class: ArtifactClass,
) -> Result<ReopenedArtifact, TransactionFileSystemError> {
    let name = decode_artifact_key(storage, &evidence.key, &class)?;
    let parent = class.directory(storage);
    let file =
        open_component(&parent, &name, OpenKind::Any).map_err(|_| artifact_changed(storage))?;
    validate_artifact(storage, &file, evidence)?;
    Ok(ReopenedArtifact {
        parent,
        name,
        file: Arc::new(file),
    })
}

pub(super) fn transaction_id_from_key(
    root: &NativeRoot,
    key: &DurableTransactionKey,
) -> Result<TransactionId, TransactionFileSystemError> {
    if key.namespace != TRANSACTION_KEY_NAMESPACE {
        return Err(TransactionFileSystemError::Unsupported {
            capability: "native transaction recovery key",
            reason: "durable transaction key namespace is not supported".to_string(),
        });
    }
    let (root_identity, remainder) = take_key_part(&key.value)?;
    if root_identity != root.identity.value {
        return Err(TransactionFileSystemError::ChangedSincePlanning {
            path: root.absolute.as_path().display().to_string(),
        });
    }
    let (transaction_id, remainder) = take_key_part(remainder)?;
    if !remainder.is_empty() {
        return Err(recovery_error(
            "decode transaction recovery key",
            "durable transaction key has trailing bytes",
        ));
    }
    let transaction_id = String::from_utf8(transaction_id.to_vec())
        .map_err(|error| recovery_error("decode transaction recovery key", &error.to_string()))?;
    Ok(TransactionId(transaction_id))
}

fn decode_artifact_key(
    storage: &NativeStorage,
    key: &DurablePathKey,
    class: &ArtifactClass,
) -> Result<OsString, TransactionFileSystemError> {
    if key.namespace != ARTIFACT_KEY_NAMESPACE {
        return Err(recovery_error(
            "decode transaction artifact key",
            "artifact key namespace is not supported",
        ));
    }
    let (root_identity, remainder) = take_key_part(&key.value)?;
    let (transaction_id, remainder) = take_key_part(remainder)?;
    let (artifact_class, remainder) = take_key_part(remainder)?;
    let (name, remainder) = take_key_part(remainder)?;
    if !remainder.is_empty()
        || root_identity != storage.root.identity.value
        || transaction_id != storage.transaction_id.0.as_bytes()
        || artifact_class != class.bytes()
    {
        return Err(recovery_error(
            "decode transaction artifact key",
            "artifact key does not match the locked transaction storage",
        ));
    }
    validate_artifact_name(name, class.bytes())?;
    Ok(OsString::from_vec(name.to_vec()))
}

fn validate_artifact_name(name: &[u8], class: &[u8]) -> Result<(), TransactionFileSystemError> {
    let expected_length = class.len() + 1 + 16;
    let valid = name.len() == expected_length
        && name.starts_with(class)
        && name.get(class.len()) == Some(&b'-')
        && name[class.len() + 1..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    if valid {
        Ok(())
    } else {
        Err(recovery_error(
            "decode transaction artifact key",
            "artifact name is not executor-generated",
        ))
    }
}

fn validate_artifact(
    storage: &NativeStorage,
    file: &File,
    expected: &DurableFileEvidence,
) -> Result<(), TransactionFileSystemError> {
    let before = file.metadata().map_err(|_| artifact_changed(storage))?;
    let root_metadata = storage
        .root
        .directory
        .metadata()
        .map_err(|_| artifact_changed(storage))?;
    if file_kind(&before) != FileKind::File
        || before.nlink() != 1
        || before.dev() != root_metadata.dev()
    {
        return Err(artifact_changed(storage));
    }
    let contents = read_bounded(
        file,
        before.size(),
        ObservationLimit {
            max_bytes: TransactionLimits::default().max_file_bytes,
        },
    )?;
    let after = file.metadata().map_err(|_| artifact_changed(storage))?;
    if !metadata_is_stable(&before, &after) || after.size() != contents.len() as u64 {
        return Err(artifact_changed(storage));
    }
    let link_count = NonZeroU64::new(after.nlink()).ok_or_else(|| artifact_changed(storage))?;
    let actual = FileEvidence {
        exact_digest: ExactBytesDigest::new(&contents),
        identity: ExecutorFileIdentity {
            namespace: "linux-dev-ino-v1".to_string(),
            value: identity_bytes(&after),
        },
        metadata: metadata_snapshot(&after),
        link_count,
        kind: file_kind(&after),
    };
    if actual == expected.evidence {
        Ok(())
    } else {
        Err(artifact_changed(storage))
    }
}

fn validate_component(component: &[u8], first: bool) -> Result<(), TransactionFileSystemError> {
    if component.is_empty()
        || component.contains(&0)
        || component.contains(&b'/')
        || matches!(component, b"." | b"..")
        || (first && component == TRANSACTION_SIDECAR_NAME)
    {
        return Err(recovery_error(
            "decode transaction recovery path",
            "durable path contains an invalid component",
        ));
    }
    Ok(())
}

fn take_key_part(value: &[u8]) -> Result<(&[u8], &[u8]), TransactionFileSystemError> {
    let Some(length) = value.get(..4) else {
        return Err(recovery_error(
            "decode transaction recovery key",
            "durable key is truncated",
        ));
    };
    let length: [u8; 4] = length.try_into().map_err(|_| {
        recovery_error(
            "decode transaction recovery key",
            "durable key length prefix has an invalid width",
        )
    })?;
    let length = u32::from_le_bytes(length) as usize;
    let end = 4_usize.checked_add(length).ok_or_else(|| {
        recovery_error(
            "decode transaction recovery key",
            "durable key length overflows the executor address space",
        )
    })?;
    let Some(part) = value.get(4..end) else {
        return Err(recovery_error(
            "decode transaction recovery key",
            "durable key length exceeds its bytes",
        ));
    };
    Ok((part, &value[end..]))
}

fn artifact_changed(storage: &NativeStorage) -> TransactionFileSystemError {
    TransactionFileSystemError::ChangedSincePlanning {
        path: format!("transaction artifact for {}", storage.transaction_id.0),
    }
}

fn recovery_error(operation: &'static str, reason: &str) -> TransactionFileSystemError {
    TransactionFileSystemError::Platform {
        operation,
        reason: reason.to_string(),
    }
}
