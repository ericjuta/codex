//! Executor-owned capability boundary for recoverable Hashline transactions.

mod capability;
mod observation;

pub use capability::CanonicalPathKey;
pub use capability::DurablePathKey;
pub use capability::DurableTransactionKey;
pub use capability::ExecutorRootIdentity;
pub use capability::GuardedMutation;
pub use capability::JournalState;
pub use capability::MutationOutcome;
pub use capability::PlanningFileSystem;
pub use capability::RecoveryOutcome;
pub use capability::StageFileRequest;
pub use capability::TransactionCoordination;
pub use capability::TransactionFileSystem;
pub use capability::TransactionFileSystemError;
pub use capability::TransactionId;
pub use capability::TransactionMutation;
pub use capability::TransactionRecovery;
pub use capability::TransactionStorage;
pub use observation::ExactBytesDigest;
pub use observation::ExecutorFileIdentity;
pub use observation::FileKind;
pub use observation::MetadataFingerprint;
pub use observation::MetadataSnapshot;
pub use observation::ObservationLimit;
pub use observation::ObservedFile;
pub use observation::ObservedPath;

#[cfg(test)]
#[path = "capability_tests.rs"]
mod tests;
