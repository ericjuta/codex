use codex_hashline_transaction::CanonicalPathKey;
use codex_hashline_transaction::ExecutorRootIdentity;
use codex_hashline_transaction::ObservationLimit;
use codex_hashline_transaction::ObservedPath;
use codex_hashline_transaction::PlanningFileSystem;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_utils_path_uri::PathUri;

use super::NativePlanningFileSystem;

#[derive(Clone, Debug)]
pub struct NativeRoot;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeResolvedPath;

impl PlanningFileSystem for NativePlanningFileSystem {
    type Root = NativeRoot;
    type ResolvedPath = NativeResolvedPath;

    async fn open_root(&self, _root: &PathUri) -> Result<Self::Root, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn resolve(
        &self,
        _root: &Self::Root,
        _model_path: &str,
    ) -> Result<Self::ResolvedPath, TransactionFileSystemError> {
        Err(unsupported())
    }

    async fn observe(
        &self,
        _path: &Self::ResolvedPath,
        _limit: ObservationLimit,
    ) -> Result<ObservedPath, TransactionFileSystemError> {
        Err(unsupported())
    }

    fn root_identity(
        &self,
        _root: &Self::Root,
    ) -> Result<ExecutorRootIdentity, TransactionFileSystemError> {
        Err(unsupported())
    }

    fn canonical_path_key(
        &self,
        _path: &Self::ResolvedPath,
    ) -> Result<CanonicalPathKey, TransactionFileSystemError> {
        Err(unsupported())
    }
}

fn unsupported() -> TransactionFileSystemError {
    TransactionFileSystemError::Unsupported {
        capability: "hashline transaction planning",
        reason: "native no-follow path handles are not implemented on this executor platform"
            .to_string(),
    }
}
