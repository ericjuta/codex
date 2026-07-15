/// Executor-native, read-only filesystem capability for Hashline transaction planning.
#[derive(Clone, Debug, Default)]
pub struct NativePlanningFileSystem;

#[cfg(target_os = "linux")]
#[path = "hashline_transaction_fs_linux.rs"]
mod platform;

#[cfg(not(target_os = "linux"))]
#[path = "hashline_transaction_fs_unsupported.rs"]
mod platform;
