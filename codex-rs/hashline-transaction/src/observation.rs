use std::fmt;
use std::num::NonZeroU64;

use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

/// SHA-256 digest of the exact bytes observed by the executor.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ExactBytesDigest([u8; 32]);

impl ExactBytesDigest {
    pub fn new(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub const fn from_array(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for ExactBytesDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Stable executor-native identity of one opened file object.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorFileIdentity {
    pub namespace: String,
    pub value: Vec<u8>,
}

/// Executor-derived fingerprint of metadata that must survive a transaction.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MetadataFingerprint([u8; 32]);

impl MetadataFingerprint {
    pub fn new(executor_canonical_metadata: &[u8]) -> Self {
        Self(Sha256::digest(executor_canonical_metadata).into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Executor-opaque metadata that can both be fingerprinted and restored.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataSnapshot {
    pub namespace: String,
    pub value: Vec<u8>,
    pub fingerprint: MetadataFingerprint,
}

impl MetadataSnapshot {
    pub fn new(namespace: String, value: Vec<u8>) -> Self {
        let fingerprint = MetadataFingerprint::new(&value);
        Self {
            namespace,
            value,
            fingerprint,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    File,
    Directory,
    SymbolicLink,
    Other,
}

/// Maximum exact file bytes an observation may retain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationLimit {
    pub max_bytes: u64,
}

/// Exact bytes and identity evidence obtained from the same guarded file object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedFile {
    pub contents: Vec<u8>,
    pub exact_digest: ExactBytesDigest,
    pub identity: ExecutorFileIdentity,
    pub metadata: MetadataSnapshot,
    pub link_count: NonZeroU64,
    pub kind: FileKind,
}

impl ObservedFile {
    pub fn new(
        contents: Vec<u8>,
        identity: ExecutorFileIdentity,
        metadata: MetadataSnapshot,
        link_count: NonZeroU64,
        kind: FileKind,
    ) -> Self {
        let exact_digest = ExactBytesDigest::new(&contents);
        Self {
            contents,
            exact_digest,
            identity,
            metadata,
            link_count,
            kind,
        }
    }
}

/// Executor observation of a resolved transaction path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservedPath {
    Absent,
    Present(ObservedFile),
}
