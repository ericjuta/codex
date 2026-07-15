use codex_utils_path_uri::PathUri;
use serde::Deserialize;
use serde::Serialize;

use crate::CanonicalPathKey;
use crate::ExactBytesDigest;
use crate::ExecutorRootIdentity;
use crate::ObservedFile;

const DEFAULT_MAX_MUTATIONS: u64 = 64;
const DEFAULT_MAX_EDITS: u64 = 1024;
const DEFAULT_MAX_EDIT_LINES: u64 = 65_536;
const DEFAULT_MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const DEFAULT_MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_MAX_MODEL_PATH_BYTES: u64 = 4096;
const DEFAULT_MAX_EXECUTOR_KEY_BYTES: u64 = 4096;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionRequest {
    pub environment_id: String,
    pub root: PathUri,
    pub action: TransactionAction,
    pub mutations: Vec<FileMutation>,
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
    pub max_edits: u64,
    pub max_edit_lines: u64,
    pub max_input_bytes: u64,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_model_path_bytes: u64,
    pub max_executor_key_bytes: u64,
}

impl Default for TransactionLimits {
    fn default() -> Self {
        Self {
            max_mutations: DEFAULT_MAX_MUTATIONS,
            max_edits: DEFAULT_MAX_EDITS,
            max_edit_lines: DEFAULT_MAX_EDIT_LINES,
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_model_path_bytes: DEFAULT_MAX_MODEL_PATH_BYTES,
            max_executor_key_bytes: DEFAULT_MAX_EXECUTOR_KEY_BYTES,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedFile {
    pub exact_digest: ExactBytesDigest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineAnchor {
    pub line: u64,
    pub expected_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineRange {
    pub start: LineAnchor,
    pub end: LineAnchor,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum FileEdit {
    ReplaceAll {
        contents: Vec<u8>,
    },
    ReplaceLines {
        range: LineRange,
        lines: Vec<String>,
    },
    InsertBefore {
        anchor: LineAnchor,
        lines: Vec<String>,
    },
    InsertAfter {
        anchor: LineAnchor,
        lines: Vec<String>,
    },
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
