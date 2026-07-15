use crate::DurableFileEvidence;
use crate::DurablePathKey;
use crate::FileEvidence;
use crate::JournalBytes;
use crate::JournalError;
use crate::JournalOperation;
use crate::JournalRecord;
use crate::MetadataFingerprint;
use crate::TransactionLimits;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JournalReadLimits {
    pub max_bytes: u64,
    pub max_mutations: u64,
    pub max_key_bytes: u64,
}

impl From<TransactionLimits> for JournalReadLimits {
    fn from(limits: TransactionLimits) -> Self {
        Self {
            max_bytes: limits.max_journal_bytes,
            max_mutations: limits.max_mutations,
            max_key_bytes: limits.max_executor_key_bytes,
        }
    }
}

impl JournalRecord {
    pub fn from_bounded_json(
        bytes: &JournalBytes,
        limits: JournalReadLimits,
    ) -> Result<Self, JournalError> {
        check_limit("bytes", bytes.len() as u64, limits.max_bytes)?;
        let record = serde_json::from_slice::<Self>(bytes.as_bytes()).map_err(|error| {
            JournalError::Serialization {
                reason: error.to_string(),
            }
        })?;
        record.validate()?;
        check_limit(
            "mutation count",
            record.mutations.len() as u64,
            limits.max_mutations,
        )?;
        validate_record_keys(&record, limits.max_key_bytes)?;
        validate_unique_paths(&record)?;
        Ok(record)
    }
}

fn validate_record_keys(record: &JournalRecord, max_bytes: u64) -> Result<(), JournalError> {
    validate_text("transactionId", &record.transaction_id.0, max_bytes)?;
    validate_opaque(
        "transactionKey",
        &record.transaction_key.namespace,
        &record.transaction_key.value,
        max_bytes,
    )?;
    validate_text("environmentId", &record.environment_id, max_bytes)?;
    validate_path_key("root", &record.root, max_bytes)?;
    validate_opaque(
        "rootIdentity",
        &record.root_identity.namespace,
        &record.root_identity.value,
        max_bytes,
    )?;
    for mutation in &record.mutations {
        validate_operation(&mutation.operation, max_bytes)?;
    }
    Ok(())
}

fn validate_operation(operation: &JournalOperation, max_bytes: u64) -> Result<(), JournalError> {
    match operation {
        JournalOperation::Create {
            destination,
            staged,
        } => {
            validate_path_key("destination", destination, max_bytes)?;
            validate_durable_file("staged", staged, max_bytes)
        }
        JournalOperation::Update {
            path,
            before,
            staged,
            backup,
        } => {
            validate_path_key("path", path, max_bytes)?;
            validate_file_evidence("before", before, max_bytes)?;
            validate_durable_file("staged", staged, max_bytes)?;
            validate_durable_file("backup", backup, max_bytes)
        }
        JournalOperation::Delete {
            path,
            before,
            backup,
        } => {
            validate_path_key("path", path, max_bytes)?;
            validate_file_evidence("before", before, max_bytes)?;
            validate_durable_file("backup", backup, max_bytes)
        }
        JournalOperation::Move {
            source,
            destination,
            before,
            staged,
            backup,
        } => {
            validate_path_key("source", source, max_bytes)?;
            validate_path_key("destination", destination, max_bytes)?;
            validate_file_evidence("before", before, max_bytes)?;
            validate_durable_file("staged", staged, max_bytes)?;
            validate_durable_file("backup", backup, max_bytes)
        }
    }
}

fn validate_durable_file(
    field: &'static str,
    file: &DurableFileEvidence,
    max_bytes: u64,
) -> Result<(), JournalError> {
    validate_path_key(field, &file.key, max_bytes)?;
    validate_file_evidence(field, &file.evidence, max_bytes)
}

fn validate_file_evidence(
    field: &'static str,
    evidence: &FileEvidence,
    max_bytes: u64,
) -> Result<(), JournalError> {
    validate_opaque(
        field,
        &evidence.identity.namespace,
        &evidence.identity.value,
        max_bytes,
    )?;
    validate_opaque(
        field,
        &evidence.metadata.namespace,
        &evidence.metadata.value,
        max_bytes,
    )?;
    if evidence.metadata.fingerprint != MetadataFingerprint::new(&evidence.metadata.value) {
        return Err(JournalError::InvalidField {
            field,
            reason: "metadata fingerprint does not match metadata bytes",
        });
    }
    Ok(())
}

fn validate_path_key(
    field: &'static str,
    key: &DurablePathKey,
    max_bytes: u64,
) -> Result<(), JournalError> {
    validate_opaque(field, &key.namespace, &key.value, max_bytes)
}

fn validate_text(field: &'static str, value: &str, max_bytes: u64) -> Result<(), JournalError> {
    if value.is_empty() {
        return Err(JournalError::InvalidField {
            field,
            reason: "value must not be empty",
        });
    }
    check_limit(field, value.len() as u64, max_bytes)
}

fn validate_opaque(
    field: &'static str,
    namespace: &str,
    value: &[u8],
    max_bytes: u64,
) -> Result<(), JournalError> {
    if namespace.is_empty() || value.is_empty() {
        return Err(JournalError::InvalidField {
            field,
            reason: "namespace and value must not be empty",
        });
    }
    check_limit(
        field,
        (namespace.len() as u64).saturating_add(value.len() as u64),
        max_bytes,
    )
}

fn check_limit(resource: &'static str, observed: u64, limit: u64) -> Result<(), JournalError> {
    if observed > limit {
        Err(JournalError::StructuralLimit {
            resource,
            observed,
            limit,
        })
    } else {
        Ok(())
    }
}

fn validate_unique_paths(record: &JournalRecord) -> Result<(), JournalError> {
    let mut paths = Vec::with_capacity(record.mutations.len().saturating_mul(2));
    for mutation in &record.mutations {
        match &mutation.operation {
            JournalOperation::Create { destination, .. } => paths.push(destination),
            JournalOperation::Update { path, .. } | JournalOperation::Delete { path, .. } => {
                paths.push(path);
            }
            JournalOperation::Move {
                source,
                destination,
                ..
            } => {
                paths.push(source);
                paths.push(destination);
            }
        }
    }
    for (index, path) in paths.iter().enumerate() {
        if paths[..index].contains(path) {
            return Err(JournalError::InvalidField {
                field: "mutations",
                reason: "durable paths must be unique",
            });
        }
    }
    Ok(())
}
