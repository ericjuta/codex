use codex_hashline_transaction::TransactionFileSystemError;
use codex_utils_path_uri::PathUri;

/// Executor-native, read-only filesystem capability for Hashline transaction planning.
#[derive(Clone, Debug, Default)]
pub struct NativePlanningFileSystem;

/// Executor-native Hashline transaction capability bound to one environment and root.
#[derive(Clone, Debug)]
pub struct NativeTransactionFileSystem {
    environment_id: String,
    root: PathUri,
}

impl NativeTransactionFileSystem {
    /// Creates a transaction capability for one stable environment identifier and root URI.
    pub fn new(environment_id: String, root: PathUri) -> Self {
        Self {
            environment_id,
            root,
        }
    }

    /// Returns the environment identifier that future recovery journals must match.
    pub fn environment_id(&self) -> &str {
        &self.environment_id
    }

    fn ensure_configured_root(&self, root: &PathUri) -> Result<(), TransactionFileSystemError> {
        if root != &self.root {
            return Err(TransactionFileSystemError::InvalidRoot {
                root: root.clone(),
                reason: format!(
                    "transaction filesystem is configured for root `{}`",
                    self.root
                ),
            });
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
#[path = "hashline_transaction_fs_linux.rs"]
mod platform;

#[cfg(not(target_os = "linux"))]
#[path = "hashline_transaction_fs_unsupported.rs"]
mod platform;
