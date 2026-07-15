use std::num::NonZeroU64;

use pretty_assertions::assert_eq;

use crate::DurableFileEvidence;
use crate::DurablePathKey;
use crate::ExactBytesDigest;
use crate::ExecutorFileIdentity;
use crate::ExecutorRootIdentity;
use crate::FileEvidence;
use crate::FileKind;
use crate::JournalError;
use crate::JournalOperation;
use crate::JournalRecord;
use crate::JournalState;
use crate::MetadataSnapshot;
use crate::MutationProgress;
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

    assert_eq!(record.state, JournalState::Complete);
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

    assert_eq!(record.state, JournalState::Complete);
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
