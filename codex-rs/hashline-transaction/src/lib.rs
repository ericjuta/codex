//! Executor-owned capability boundary for recoverable Hashline transactions.

mod capability;
mod edits;
mod executor;
mod journal;
mod journal_decode;
mod journal_state;
mod limits;
mod observation;
mod planner;
mod prepared;
mod preview;
mod recovered;
mod recovery;
mod recovery_rollback;
mod recovery_verify;
mod rollback;
mod types;

pub use capability::CanonicalPathKey;
pub use capability::DurablePathKey;
pub use capability::DurableTransactionKey;
pub use capability::ExecutorRootIdentity;
pub use capability::GuardedMutation;
pub use capability::GuardedRollback;
pub use capability::LoadedJournal;
pub use capability::MutationOutcome;
pub use capability::ObservedEvidence;
pub use capability::PlanningFileSystem;
pub use capability::RecoveryScanLimit;
pub use capability::StageFileRequest;
pub use capability::TransactionCoordination;
pub use capability::TransactionFileSystem;
pub use capability::TransactionFileSystemError;
pub use capability::TransactionId;
pub use capability::TransactionMutation;
pub use capability::TransactionRecovery;
pub use capability::TransactionStorage;
pub use executor::ExecuteError;
pub use executor::ExecutionFailure;
pub use executor::ExecutionOutcome;
pub use executor::ExecutionResult;
pub use executor::execute;
pub use journal::DurableFileEvidence;
pub use journal::FileEvidence;
pub use journal::JournalBytes;
pub use journal::JournalError;
pub use journal::JournalMutation;
pub use journal::JournalOperation;
pub use journal::JournalRecord;
pub use journal::JournalState;
pub use journal::MutationProgress;
pub use journal::RecoveryTarget;
pub use journal::StorageRequirements;
pub use journal::TRANSACTION_JOURNAL_SCHEMA_VERSION;
pub use journal_decode::JournalReadLimits;
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
pub use recovery::RecoveryAttempt;
pub use recovery::RecoveryError;
pub use recovery::RecoveryFailure;
pub use recovery::RecoveryOutcome;
pub use recovery::RecoveryResult;
pub use recovery::recover_pending;
pub use recovery::recover_transaction;
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

#[cfg(test)]
#[path = "journal_tests.rs"]
mod journal_tests;

#[cfg(test)]
#[path = "executor_test_support.rs"]
mod executor_test_support;

#[cfg(test)]
#[path = "executor_test_mutation.rs"]
mod executor_test_mutation;

#[cfg(test)]
#[path = "executor_test_recovery.rs"]
mod executor_test_recovery;

#[cfg(test)]
#[path = "executor_tests.rs"]
mod executor_tests;

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod recovery_tests;

#[cfg(test)]
#[path = "recovery_before_apply_tests.rs"]
mod recovery_before_apply_tests;

#[cfg(test)]
#[path = "recovery_scan_tests.rs"]
mod recovery_scan_tests;

#[cfg(test)]
#[path = "recovery_storage_tests.rs"]
mod recovery_storage_tests;

#[cfg(test)]
#[path = "recovery_terminal_tests.rs"]
mod recovery_terminal_tests;
