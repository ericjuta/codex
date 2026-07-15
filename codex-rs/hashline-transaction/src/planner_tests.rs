use std::collections::BTreeMap;
use std::num::NonZeroU64;

use codex_utils_path_uri::PathUri;
use futures::executor::block_on;
use pretty_assertions::assert_eq;

use super::*;

#[derive(Debug, Default)]
struct FakePlanningFileSystem {
    files: BTreeMap<String, ObservedPath>,
    aliases: BTreeMap<String, String>,
}

impl PlanningFileSystem for FakePlanningFileSystem {
    type Root = String;
    type ResolvedPath = String;

    async fn open_root(&self, root: &PathUri) -> Result<Self::Root, TransactionFileSystemError> {
        Ok(root.to_string())
    }

    async fn resolve(
        &self,
        _root: &Self::Root,
        model_path: &str,
    ) -> Result<Self::ResolvedPath, TransactionFileSystemError> {
        Ok(self
            .aliases
            .get(model_path)
            .cloned()
            .unwrap_or_else(|| model_path.to_string()))
    }

    async fn observe(
        &self,
        path: &Self::ResolvedPath,
        _limit: ObservationLimit,
    ) -> Result<ObservedPath, TransactionFileSystemError> {
        Ok(self
            .files
            .get(path)
            .cloned()
            .unwrap_or(ObservedPath::Absent))
    }

    fn root_identity(
        &self,
        root: &Self::Root,
    ) -> Result<ExecutorRootIdentity, TransactionFileSystemError> {
        Ok(ExecutorRootIdentity {
            namespace: "fake-root".to_string(),
            value: root.as_bytes().to_vec(),
        })
    }

    fn canonical_path_key(
        &self,
        path: &Self::ResolvedPath,
    ) -> Result<CanonicalPathKey, TransactionFileSystemError> {
        Ok(CanonicalPathKey {
            namespace: "fake-path".to_string(),
            value: path.as_bytes().to_vec(),
        })
    }
}

fn observed(contents: &[u8], identity: u8) -> ObservedPath {
    ObservedPath::Present(ObservedFile::new(
        contents.to_vec(),
        ExecutorFileIdentity {
            namespace: "fake-file".to_string(),
            value: vec![identity],
        },
        MetadataSnapshot::new("fake-metadata".to_string(), b"mode=0644".to_vec()),
        NonZeroU64::MIN,
        FileKind::File,
    ))
}

fn request(mutations: Vec<FileMutation>) -> TransactionRequest {
    TransactionRequest {
        environment_id: "test-env".to_string(),
        root: PathUri::parse("file:///workspace").unwrap(),
        action: TransactionAction::Preview,
        mutations,
        limits: TransactionLimits::default(),
    }
}

fn expected(contents: &[u8]) -> ExpectedFile {
    ExpectedFile {
        exact_digest: ExactBytesDigest::new(contents),
    }
}

#[test]
fn mixed_plan_has_complete_summary_and_order_independent_digest() {
    let file_system = FakePlanningFileSystem {
        files: BTreeMap::from([
            ("update".to_string(), observed(b"old-u", 1)),
            ("delete".to_string(), observed(b"old-d", 2)),
            ("move".to_string(), observed(b"old-m", 3)),
        ]),
        aliases: BTreeMap::new(),
    };
    let mutations = vec![
        FileMutation::Create {
            path: "create".to_string(),
            contents: b"created".to_vec(),
        },
        FileMutation::Update {
            path: "update".to_string(),
            expected: expected(b"old-u"),
            edits: vec![FileEdit::ReplaceAll {
                contents: b"updated".to_vec(),
            }],
        },
        FileMutation::Delete {
            path: "delete".to_string(),
            expected: expected(b"old-d"),
        },
        FileMutation::Move {
            source: "move".to_string(),
            expected: expected(b"old-m"),
            destination: "moved".to_string(),
            edits: Vec::new(),
        },
    ];

    let first = block_on(plan(&file_system, request(mutations.clone()))).unwrap();
    let second = block_on(plan(
        &file_system,
        request(mutations.into_iter().rev().collect()),
    ))
    .unwrap();

    assert_eq!(
        first.summary,
        PlanSummary {
            creates: 1,
            updates: 1,
            deletes: 1,
            moves: 1,
            before_bytes: 15,
            after_bytes: 19,
        }
    );
    assert_eq!(first.plan_digest, second.plan_digest);
}
