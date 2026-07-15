use std::collections::BTreeMap;
use std::num::NonZeroU64;

use codex_utils_path_uri::PathUri;
use futures::executor::block_on;
use pretty_assertions::assert_eq;

use super::*;

#[derive(Clone, Debug, Default)]
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

fn observed_file(contents: &[u8], identity: u8) -> ObservedFile {
    ObservedFile::new(
        contents.to_vec(),
        ExecutorFileIdentity {
            namespace: "fake-file".to_string(),
            value: vec![identity],
        },
        MetadataSnapshot::new("fake-metadata".to_string(), b"mode=0644".to_vec()),
        NonZeroU64::MIN,
        FileKind::File,
    )
}

fn observed(contents: &[u8], identity: u8) -> ObservedPath {
    ObservedPath::Present(observed_file(contents, identity))
}

fn request(mutations: Vec<FileMutation>) -> TransactionRequest {
    TransactionRequest {
        environment_id: "test-env".to_string(),
        root: PathUri::parse("file:///workspace").unwrap(),
        action: TransactionAction::Preview,
        mutations,
    }
}

fn expected(contents: &[u8]) -> ExpectedFile {
    ExpectedFile {
        exact_digest: ExactBytesDigest::new(contents),
    }
}

fn create(path: &str, contents: &[u8]) -> FileMutation {
    FileMutation::Create {
        path: path.to_string(),
        contents: contents.to_vec(),
    }
}

fn update(path: &str, before: &[u8], after: &[u8]) -> FileMutation {
    FileMutation::Update {
        path: path.to_string(),
        expected: expected(before),
        edits: vec![FileEdit::ReplaceAll {
            contents: after.to_vec(),
        }],
    }
}

fn plan_error(file_system: &FakePlanningFileSystem, request: TransactionRequest) -> PlanError {
    block_on(plan(file_system, request)).unwrap_err()
}

fn plan_error_with_limits(
    file_system: &FakePlanningFileSystem,
    request: TransactionRequest,
    limits: TransactionLimits,
) -> PlanError {
    block_on(plan_with_limits(file_system, request, limits)).unwrap_err()
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

#[test]
fn rejects_aliases_and_invalid_file_preconditions() {
    let aliased = FakePlanningFileSystem {
        aliases: BTreeMap::from([
            ("first".to_string(), "same".to_string()),
            ("second".to_string(), "same".to_string()),
        ]),
        ..Default::default()
    };
    assert_eq!(
        plan_error(
            &aliased,
            request(vec![create("first", b"1"), create("second", b"2")]),
        ),
        PlanError::PathConflict {
            first: "first".to_string(),
            second: "second".to_string(),
        }
    );

    let existing = FakePlanningFileSystem {
        files: BTreeMap::from([("file".to_string(), observed(b"old", 1))]),
        ..Default::default()
    };
    assert_eq!(
        plan_error(&existing, request(vec![create("file", b"new")])),
        PlanError::ExpectedAbsent {
            path: "file".to_string(),
        }
    );
    assert_eq!(
        plan_error(
            &FakePlanningFileSystem::default(),
            request(vec![update("missing", b"old", b"new")]),
        ),
        PlanError::ExpectedExistingFile {
            path: "missing".to_string(),
        }
    );
    assert_eq!(
        plan_error(&existing, request(vec![update("file", b"stale", b"new")])),
        PlanError::Stale {
            path: "file".to_string(),
        }
    );
}

#[test]
fn rejects_unsupported_kinds_hard_links_and_existing_move_destinations() {
    let mut directory = observed_file(b"old", 1);
    directory.kind = FileKind::Directory;
    let directory_file_system = FakePlanningFileSystem {
        files: BTreeMap::from([("file".to_string(), ObservedPath::Present(directory))]),
        ..Default::default()
    };
    assert_eq!(
        plan_error(
            &directory_file_system,
            request(vec![update("file", b"old", b"new")]),
        ),
        PlanError::UnsupportedKind {
            path: "file".to_string(),
            kind: FileKind::Directory,
        }
    );

    let mut linked = observed_file(b"old", 1);
    linked.link_count = NonZeroU64::new(2).unwrap();
    let linked_file_system = FakePlanningFileSystem {
        files: BTreeMap::from([("file".to_string(), ObservedPath::Present(linked))]),
        ..Default::default()
    };
    assert_eq!(
        plan_error(
            &linked_file_system,
            request(vec![update("file", b"old", b"new")]),
        ),
        PlanError::HardLink {
            path: "file".to_string(),
            link_count: 2,
        }
    );

    let move_file_system = FakePlanningFileSystem {
        files: BTreeMap::from([
            ("source".to_string(), observed(b"old", 1)),
            ("destination".to_string(), observed(b"occupied", 2)),
        ]),
        ..Default::default()
    };
    assert_eq!(
        plan_error(
            &move_file_system,
            request(vec![FileMutation::Move {
                source: "source".to_string(),
                expected: expected(b"old"),
                destination: "destination".to_string(),
                edits: Vec::new(),
            }]),
        ),
        PlanError::ExpectedAbsent {
            path: "destination".to_string(),
        }
    );
}

#[test]
fn digest_binds_file_identity_and_metadata() {
    let base = FakePlanningFileSystem {
        files: BTreeMap::from([("file".to_string(), observed(b"old", 1))]),
        ..Default::default()
    };
    let mut replaced_identity = base.clone();
    let Some(ObservedPath::Present(file)) = replaced_identity.files.get_mut("file") else {
        unreachable!();
    };
    file.identity.value = vec![2];
    let mut changed_metadata = base.clone();
    let Some(ObservedPath::Present(file)) = changed_metadata.files.get_mut("file") else {
        unreachable!();
    };
    file.metadata = MetadataSnapshot::new("fake-metadata".to_string(), b"mode=0600".to_vec());

    let mutation = update("file", b"old", b"new");
    let base_plan = block_on(plan(&base, request(vec![mutation.clone()]))).unwrap();
    let identity_plan =
        block_on(plan(&replaced_identity, request(vec![mutation.clone()]))).unwrap();
    let metadata_plan = block_on(plan(&changed_metadata, request(vec![mutation]))).unwrap();

    assert_ne!(base_plan.plan_digest, identity_plan.plan_digest);
    assert_ne!(base_plan.plan_digest, metadata_plan.plan_digest);
}

#[test]
fn commit_previewed_requires_the_current_plan_digest() {
    let file_system = FakePlanningFileSystem::default();
    let mutation = create("file", b"new");
    let preview = block_on(plan(&file_system, request(vec![mutation.clone()]))).unwrap();

    let mut matching = request(vec![mutation.clone()]);
    matching.action = TransactionAction::CommitPreviewed {
        expected_plan_digest: preview.plan_digest,
    };
    let committed = block_on(plan(&file_system, matching)).unwrap();
    assert_eq!(committed.plan_digest, preview.plan_digest);

    let expected = ExactBytesDigest::new(b"not-the-plan");
    let mut stale = request(vec![mutation]);
    stale.action = TransactionAction::CommitPreviewed {
        expected_plan_digest: expected,
    };
    assert_eq!(
        plan_error(&file_system, stale),
        PlanError::PlanDigestMismatch {
            expected,
            actual: preview.plan_digest,
        }
    );
}

#[test]
fn rejects_empty_and_ambiguous_edit_lists() {
    let file_system = FakePlanningFileSystem {
        files: BTreeMap::from([("file".to_string(), observed(b"old", 1))]),
        ..Default::default()
    };
    assert_eq!(
        plan_error(&file_system, request(Vec::new())),
        PlanError::Empty
    );

    assert_eq!(
        plan_error(
            &file_system,
            request(vec![FileMutation::Update {
                path: "file".to_string(),
                expected: expected(b"old"),
                edits: Vec::new(),
            }]),
        ),
        PlanError::InvalidEdits {
            path: "file".to_string(),
        }
    );
    assert_eq!(
        plan_error(
            &file_system,
            request(vec![FileMutation::Update {
                path: "file".to_string(),
                expected: expected(b"old"),
                edits: vec![
                    FileEdit::ReplaceAll {
                        contents: b"one".to_vec(),
                    },
                    FileEdit::ReplaceAll {
                        contents: b"two".to_vec(),
                    },
                ],
            }]),
        ),
        PlanError::InvalidEdits {
            path: "file".to_string(),
        }
    );
}

#[test]
fn enforces_every_planner_limit() {
    let empty = FakePlanningFileSystem::default();

    let mutation_count = request(vec![create("one", b"1"), create("two", b"2")]);
    let limits = TransactionLimits {
        max_mutations: 1,
        ..Default::default()
    };
    assert_eq!(
        plan_error_with_limits(&empty, mutation_count, limits),
        PlanError::Limit {
            resource: "mutation count",
            observed: 2,
            limit: 1,
        }
    );

    let edit_count = request(vec![FileMutation::Update {
        path: "file".to_string(),
        expected: expected(b"old"),
        edits: vec![
            FileEdit::ReplaceAll {
                contents: b"one".to_vec(),
            },
            FileEdit::ReplaceAll {
                contents: b"two".to_vec(),
            },
        ],
    }]);
    let limits = TransactionLimits {
        max_edits: 1,
        ..Default::default()
    };
    assert_eq!(
        plan_error_with_limits(&empty, edit_count, limits),
        PlanError::Limit {
            resource: "edit count",
            observed: 2,
            limit: 1,
        }
    );

    let edit_lines = request(vec![FileMutation::Update {
        path: "file".to_string(),
        expected: expected(b"old"),
        edits: vec![FileEdit::InsertBefore {
            anchor: LineAnchor {
                line: 1,
                expected_hash: "0000".to_string(),
            },
            lines: vec![String::new(), String::new()],
        }],
    }]);
    let limits = TransactionLimits {
        max_edit_lines: 1,
        ..Default::default()
    };
    assert_eq!(
        plan_error_with_limits(&empty, edit_lines, limits),
        PlanError::Limit {
            resource: "edit line count",
            observed: 2,
            limit: 1,
        }
    );

    let input_bytes = request(vec![create("file", b"new")]);
    let observed_input_bytes = super::limits::request_costs(&input_bytes).input_bytes;
    let limits = TransactionLimits {
        max_input_bytes: observed_input_bytes - 1,
        ..Default::default()
    };
    assert_eq!(
        plan_error_with_limits(&empty, input_bytes, limits),
        PlanError::Limit {
            resource: "input bytes",
            observed: observed_input_bytes,
            limit: observed_input_bytes - 1,
        }
    );

    let model_path = request(vec![create("long", b"1")]);
    let limits = TransactionLimits {
        max_model_path_bytes: 3,
        ..Default::default()
    };
    assert_eq!(
        plan_error_with_limits(&empty, model_path, limits),
        PlanError::Limit {
            resource: "model path bytes",
            observed: 4,
            limit: 3,
        }
    );

    let root_key = request(vec![create("file", b"1")]);
    let root_key_bytes = "fake-root".len() as u64 + root_key.root.to_string().len() as u64;
    let limits = TransactionLimits {
        max_executor_key_bytes: root_key_bytes - 1,
        ..Default::default()
    };
    assert_eq!(
        plan_error_with_limits(&empty, root_key, limits),
        PlanError::Limit {
            resource: "executor key bytes",
            observed: root_key_bytes,
            limit: root_key_bytes - 1,
        }
    );

    let long_path = "x".repeat(40);
    let path_key = request(vec![create(&long_path, b"1")]);
    let limits = TransactionLimits {
        max_executor_key_bytes: 30,
        ..Default::default()
    };
    assert_eq!(
        plan_error_with_limits(&empty, path_key, limits),
        PlanError::Limit {
            resource: "executor key bytes",
            observed: 49,
            limit: 30,
        }
    );

    let existing = FakePlanningFileSystem {
        files: BTreeMap::from([("file".to_string(), observed(b"old", 1))]),
        ..Default::default()
    };
    let before_file = request(vec![update("file", b"old", b"new")]);
    let limits = TransactionLimits {
        max_file_bytes: 2,
        ..Default::default()
    };
    assert_eq!(
        plan_error_with_limits(&existing, before_file, limits),
        PlanError::Limit {
            resource: "file bytes",
            observed: 3,
            limit: 2,
        }
    );

    let after_file = request(vec![create("file", b"new")]);
    let limits = TransactionLimits {
        max_file_bytes: 2,
        ..Default::default()
    };
    assert_eq!(
        plan_error_with_limits(&empty, after_file, limits),
        PlanError::Limit {
            resource: "file bytes",
            observed: 3,
            limit: 2,
        }
    );

    let total_file_system = FakePlanningFileSystem {
        files: BTreeMap::from([("file".to_string(), observed(b"old", 1))]),
        ..Default::default()
    };
    let total = request(vec![update("file", b"old", b"new")]);
    let limits = TransactionLimits {
        max_total_bytes: 5,
        ..Default::default()
    };
    assert_eq!(
        plan_error_with_limits(&total_file_system, total, limits),
        PlanError::Limit {
            resource: "total bytes",
            observed: 6,
            limit: 5,
        }
    );
}
