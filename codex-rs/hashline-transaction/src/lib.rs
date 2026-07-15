//! Executor-owned capability boundary for recoverable Hashline transactions.

mod capability;
mod edits;
mod limits;
mod observation;
mod planner;
mod preview;
mod types;

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
pub use planner::PlanError;
pub use planner::plan;
pub use planner::plan_with_limits;
pub use preview::MutationPreview;
pub use preview::PlanPreview;
pub use preview::PreviewText;
pub use preview::build_preview;
pub use types::ExpectedFile;
pub use types::FileEdit;
pub use types::FileMutation;
pub use types::LineAnchor;
pub use types::LineRange;
pub use types::PlanSummary;
pub use types::PlannedMutation;
pub use types::PlannedTransaction;
pub use types::TransactionAction;
pub use types::TransactionLimits;
pub use types::TransactionRequest;

#[cfg(test)]
#[path = "capability_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "planner_tests.rs"]
mod planner_tests;

#[cfg(test)]
#[path = "edits_tests.rs"]
mod edits_tests;

#[cfg(test)]
#[path = "preview_tests.rs"]
mod preview_tests;
