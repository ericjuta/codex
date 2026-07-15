use std::num::NonZeroU64;

use pretty_assertions::assert_eq;

use crate::DurableFileEvidence;
use crate::DurablePathKey;
use crate::DurableTransactionKey;
use crate::ExactBytesDigest;
use crate::ExecutorFileIdentity;
use crate::ExecutorRootIdentity;
use crate::FileEvidence;
use crate::FileKind;
use crate::JournalError;
use crate::JournalOperation;
use crate::JournalReadLimits;
use crate::JournalRecord;
use crate::JournalState;
use crate::MetadataSnapshot;
use crate::MutationProgress;
use crate::RecoveryTarget;
use crate::StorageRequirements;
use crate::TRANSACTION_JOURNAL_SCHEMA_VERSION;
use crate::TransactionId;

fn path_key(value: &str) -> DurablePathKey {
    DurablePathKey {
        namespace: "test-path-v1".to_string(),
        value: value.as_bytes().to_vec(),
    }
}

fn evidence(contents: &[u8], identity: &str) -> FileEvidence {
    FileEvidence {
        exact_digest: ExactBytesDigest::new(contents),
        identity: ExecutorFileIdentity {
            namespace: "test-file-v1".to_string(),
            value: identity.as_bytes().to_vec(),
        },
        metadata: MetadataSnapshot::new("test-metadata-v1".to_string(), b"mode:644".to_vec()),
        link_count: NonZeroU64::MIN,
        kind: FileKind::File,
    }
}

fn durable_file(path: &str, contents: &[u8], identity: &str) -> DurableFileEvidence {
    DurableFileEvidence {
        key: path_key(path),
        evidence: evidence(contents, identity),
    }
}

fn record() -> JournalRecord {
    JournalRecord::new(
        TransactionId("tx-1".to_string()),
        DurableTransactionKey {
            namespace: "test-transaction-v1".to_string(),
            value: b"tx-1".to_vec(),
        },
        "local".to_string(),
        path_key("."),
        ExecutorRootIdentity {
            namespace: "test-root-v1".to_string(),
            value: b"root-1".to_vec(),
        },
        ExactBytesDigest::new(b"plan"),
        vec![
            JournalOperation::Create {
                destination: path_key("created.txt"),
                staged: durable_file("stage/0", b"after", "stage-0"),
            },
            JournalOperation::Update {
                path: path_key("updated.txt"),
                before: evidence(b"before-file-secret", "before-1"),
                staged: durable_file("stage/1", b"after-file-secret", "stage-1"),
                backup: durable_file("backup/1", b"before-file-secret", "backup-1"),
            },
        ],
    )
    .unwrap()
}

#[test]
fn journal_round_trips_without_file_contents() {
    let record = record();

    let serialized = record.to_bounded_json(64 * 1024).unwrap();
    let decoded = serde_json::from_slice::<JournalRecord>(serialized.as_bytes()).unwrap();

    assert_eq!(decoded, record);
    assert_eq!(decoded.schema_version, TRANSACTION_JOURNAL_SCHEMA_VERSION);
    assert!(
        !serialized
            .as_bytes()
            .windows(b"before-file-secret".len())
            .any(|bytes| bytes == b"before-file-secret")
    );
    assert!(
        !serialized
            .as_bytes()
            .windows(b"after-file-secret".len())
            .any(|bytes| bytes == b"after-file-secret")
    );
}

#[test]
fn journal_enforces_state_and_mutation_progress() {
    let mut record = record();

    assert_eq!(record.recovery_target, RecoveryTarget::Rollback);

    record.transition(JournalState::Prepared).unwrap();
    record.transition(JournalState::Committing).unwrap();
    for index in 0..record.mutations.len() {
        record
            .set_mutation_progress(index, MutationProgress::Committing)
            .unwrap();
        record
            .set_mutation_progress(index, MutationProgress::Applied)
            .unwrap();
    }
    record.transition(JournalState::Committed).unwrap();
    record.transition(JournalState::Cleaning).unwrap();
    record.transition(JournalState::Complete).unwrap();

    assert_eq!(
        (record.state, record.recovery_target),
        (JournalState::Complete, RecoveryTarget::Commit)
    );
    assert_eq!(
        record
            .mutations
            .iter()
            .map(|mutation| mutation.progress)
            .collect::<Vec<_>>(),
        vec![MutationProgress::Applied, MutationProgress::Applied]
    );
}

#[test]
fn journal_rejects_skipped_states_and_inconsistent_terminal_progress() {
    let mut record = record();

    assert_eq!(
        record.transition(JournalState::Committing),
        Err(JournalError::InvalidStateTransition {
            current: JournalState::Preparing,
            next: JournalState::Committing,
        })
    );
    record.transition(JournalState::Prepared).unwrap();
    record.transition(JournalState::Committing).unwrap();
    assert_eq!(
        record.transition(JournalState::Committed),
        Err(JournalError::InconsistentProgress {
            state: JournalState::Committed,
        })
    );
}

#[test]
fn journal_rejects_unknown_schema_versions() {
    let mut record = record();
    record.schema_version += 1;

    assert_eq!(
        record.validate(),
        Err(JournalError::UnsupportedSchema {
            version: TRANSACTION_JOURNAL_SCHEMA_VERSION + 1,
        })
    );
}

#[test]
fn journal_rejects_empty_or_truncated_mutation_manifests() {
    let mut empty = record();
    empty.mutations.clear();
    assert_eq!(
        empty.validate(),
        Err(JournalError::InvalidField {
            field: "mutations",
            reason: "at least one mutation is required",
        })
    );

    let mut truncated = record();
    truncated.mutations.pop();
    assert_eq!(
        truncated.validate(),
        Err(JournalError::InvalidField {
            field: "manifestDigest",
            reason: "digest does not match the immutable transaction manifest",
        })
    );
}

#[test]
fn rollback_can_resume_a_mutation_interrupted_during_commit() {
    let mut record = record();
    record.transition(JournalState::Prepared).unwrap();
    record.transition(JournalState::Committing).unwrap();
    record
        .set_mutation_progress(0, MutationProgress::Committing)
        .unwrap();

    record.transition(JournalState::RollingBack).unwrap();
    record
        .set_mutation_progress(0, MutationProgress::RollingBack)
        .unwrap();
    record
        .set_mutation_progress(0, MutationProgress::RolledBack)
        .unwrap();

    assert_eq!(record.state, JournalState::RollingBack);
}

#[test]
fn rollback_progress_is_reverse_safe_and_terminally_consistent() {
    let mut record = record();
    record.transition(JournalState::Prepared).unwrap();
    record.transition(JournalState::Committing).unwrap();
    for index in 0..record.mutations.len() {
        record
            .set_mutation_progress(index, MutationProgress::Committing)
            .unwrap();
        record
            .set_mutation_progress(index, MutationProgress::Applied)
            .unwrap();
    }
    record.transition(JournalState::RollingBack).unwrap();
    for index in (0..record.mutations.len()).rev() {
        record
            .set_mutation_progress(index, MutationProgress::RollingBack)
            .unwrap();
        record
            .set_mutation_progress(index, MutationProgress::RolledBack)
            .unwrap();
    }
    record.transition(JournalState::RolledBack).unwrap();
    record.transition(JournalState::Cleaning).unwrap();
    record.transition(JournalState::Complete).unwrap();

    assert_eq!(
        (record.state, record.recovery_target),
        (JournalState::Complete, RecoveryTarget::Rollback)
    );
}

#[test]
fn journal_rejects_a_terminal_state_with_the_wrong_recovery_target() {
    let mut record = record();
    record.recovery_target = RecoveryTarget::Commit;

    assert_eq!(
        record.validate(),
        Err(JournalError::InconsistentRecoveryTarget {
            state: JournalState::Preparing,
            target: RecoveryTarget::Commit,
        })
    );
}

#[test]
fn recovery_required_resumes_only_toward_its_durable_target() {
    let mut rollback = record();
    rollback.transition(JournalState::RecoveryRequired).unwrap();
    rollback.transition(JournalState::RollingBack).unwrap();

    let mut commit = record();
    commit.transition(JournalState::Prepared).unwrap();
    commit.transition(JournalState::Committing).unwrap();
    for index in 0..commit.mutations.len() {
        commit
            .set_mutation_progress(index, MutationProgress::Committing)
            .unwrap();
        commit
            .set_mutation_progress(index, MutationProgress::Applied)
            .unwrap();
    }
    commit.transition(JournalState::Committed).unwrap();
    commit.transition(JournalState::RecoveryRequired).unwrap();

    assert_eq!(
        commit.transition(JournalState::RollingBack),
        Err(JournalError::InconsistentRecoveryTarget {
            state: JournalState::RollingBack,
            target: RecoveryTarget::Commit,
        })
    );
    commit.transition(JournalState::Cleaning).unwrap();
}

#[test]
fn journal_serialization_and_storage_requirements_are_bounded() {
    let record = record();
    let observed = record.to_bounded_json(u64::MAX).unwrap().len() as u64;

    assert_eq!(
        record.to_bounded_json(observed - 1),
        Err(JournalError::Limit {
            observed,
            limit: observed - 1,
        })
    );
    assert_eq!(
        StorageRequirements {
            staged_bytes: 7,
            backup_bytes: 11,
            journal_bytes: 13,
        }
        .total_bytes(),
        31
    );
}

#[test]
fn bounded_journal_decode_validates_structure() {
    let record = record();
    let bytes = record.to_bounded_json(64 * 1024).unwrap();

    assert_eq!(
        JournalRecord::from_bounded_json(
            &bytes,
            JournalReadLimits {
                max_bytes: 64 * 1024,
                max_mutations: 2,
                max_key_bytes: 4096,
            },
        )
        .unwrap(),
        record
    );
    assert_eq!(
        JournalRecord::from_bounded_json(
            &bytes,
            JournalReadLimits {
                max_bytes: 64 * 1024,
                max_mutations: 1,
                max_key_bytes: 4096,
            },
        ),
        Err(JournalError::StructuralLimit {
            resource: "mutation count",
            observed: 2,
            limit: 1,
        })
    );
}

#[test]
fn bounded_journal_decode_rejects_duplicate_paths() {
    let record = record();
    let mut operations = record
        .mutations
        .into_iter()
        .map(|mutation| mutation.operation)
        .collect::<Vec<_>>();
    let JournalOperation::Update { path, .. } = &mut operations[1] else {
        panic!("expected update operation");
    };
    *path = path_key("created.txt");
    let record = JournalRecord::new(
        record.transaction_id,
        record.transaction_key,
        record.environment_id,
        record.root,
        record.root_identity,
        record.plan_digest,
        operations,
    )
    .unwrap();
    let bytes = record.to_bounded_json(64 * 1024).unwrap();

    assert_eq!(
        JournalRecord::from_bounded_json(
            &bytes,
            JournalReadLimits {
                max_bytes: 64 * 1024,
                max_mutations: 2,
                max_key_bytes: 4096,
            },
        ),
        Err(JournalError::InvalidField {
            field: "mutations",
            reason: "durable paths must be unique",
        })
    );
}
