use codex_utils_path_uri::PathUri;
use serde::Deserialize;
use serde::Serialize;

use crate::CanonicalPathKey;
use crate::ExactBytesDigest;
use crate::ExecutorRootIdentity;
use crate::ObservedFile;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionRequest {
    pub environment_id: String,
    pub root: PathUri,
    pub action: TransactionAction,
    pub mutations: Vec<FileMutation>,
    pub limits: TransactionLimits,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum TransactionAction {
    Preview,
    Commit,
    CommitPreviewed {
        expected_plan_digest: ExactBytesDigest,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionLimits {
    pub max_mutations: u64,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_model_path_bytes: u64,
    pub max_executor_key_bytes: u64,
}

impl Default for TransactionLimits {
    fn default() -> Self {
        Self {
            max_mutations: 64,
            max_file_bytes: 4 * 1024 * 1024,
            max_total_bytes: 16 * 1024 * 1024,
            max_model_path_bytes: 4096,
            max_executor_key_bytes: 4096,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedFile {
    pub exact_digest: ExactBytesDigest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum FileEdit {
    ReplaceAll { contents: Vec<u8> },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum FileMutation {
    Create {
        path: String,
        contents: Vec<u8>,
    },
    Update {
        path: String,
        expected: ExpectedFile,
        edits: Vec<FileEdit>,
    },
    Delete {
        path: String,
        expected: ExpectedFile,
    },
    Move {
        source: String,
        expected: ExpectedFile,
        destination: String,
        edits: Vec<FileEdit>,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSummary {
    pub creates: u64,
    pub updates: u64,
    pub deletes: u64,
    pub moves: u64,
    pub before_bytes: u64,
    pub after_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedTransaction<R, P> {
    pub environment_id: String,
    pub root_uri: PathUri,
    pub root: R,
    pub root_identity: ExecutorRootIdentity,
    pub action: TransactionAction,
    pub mutations: Vec<PlannedMutation<P>>,
    pub plan_digest: ExactBytesDigest,
    pub summary: PlanSummary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlannedMutation<P> {
    Create {
        path: P,
        path_key: CanonicalPathKey,
        contents: Vec<u8>,
        after_digest: ExactBytesDigest,
    },
    Update {
        path: P,
        path_key: CanonicalPathKey,
        before: ObservedFile,
        contents: Vec<u8>,
        after_digest: ExactBytesDigest,
    },
    Delete {
        path: P,
        path_key: CanonicalPathKey,
        before: ObservedFile,
    },
    Move {
        source: P,
        source_key: CanonicalPathKey,
        before: ObservedFile,
        destination: P,
        destination_key: CanonicalPathKey,
        contents: Vec<u8>,
        after_digest: ExactBytesDigest,
    },
}
