use std::num::NonZeroU64;

use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;

use super::*;

fn path_key(path: &str) -> CanonicalPathKey {
    CanonicalPathKey {
        namespace: "test-path".to_string(),
        value: path.as_bytes().to_vec(),
    }
}

fn observed_file(contents: &[u8], identity: u8) -> ObservedFile {
    ObservedFile::new(
        contents.to_vec(),
        ExecutorFileIdentity {
            namespace: "test-file".to_string(),
            value: vec![identity],
        },
        MetadataSnapshot::new("test-metadata".to_string(), b"mode=0644".to_vec()),
        NonZeroU64::MIN,
        FileKind::File,
    )
}

fn planned_transaction(
    mutations: Vec<PlannedMutation<String>>,
) -> PlannedTransaction<String, String> {
    PlannedTransaction {
        environment_id: "test-env".to_string(),
        root_uri: PathUri::parse("file:///workspace").unwrap(),
        root: "opened-root-handle".to_string(),
        root_identity: ExecutorRootIdentity {
            namespace: "test-root".to_string(),
            value: b"native-root-identity".to_vec(),
        },
        action: TransactionAction::Preview,
        mutations,
        plan_digest: ExactBytesDigest::new(b"deterministic-plan"),
        summary: PlanSummary {
            creates: 1,
            updates: 1,
            deletes: 1,
            moves: 1,
            before_bytes: 38,
            after_bytes: 33,
        },
    }
}

fn create(path: &str, contents: &[u8]) -> PlannedMutation<String> {
    PlannedMutation::Create {
        path: format!("native:{path}"),
        model_path: path.to_string(),
        path_key: path_key(path),
        contents: contents.to_vec(),
        after_digest: ExactBytesDigest::new(contents),
    }
}

#[test]
fn exact_digest_uses_a_fixed_width_hex_wire_representation() {
    let digest = ExactBytesDigest::new(b"exact bytes");
    let encoded = serde_json::to_string(&digest).unwrap();
    assert_eq!(encoded, format!("\"{digest}\""));
    assert_eq!(encoded.len(), 66);

    let uppercase = format!("\"{}\"", digest.to_string().to_uppercase());
    let decoded = serde_json::from_str::<ExactBytesDigest>(&uppercase).unwrap();
    assert_eq!(decoded, digest);

    let error = serde_json::from_str::<ExactBytesDigest>("\"abcd\"").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("expected exactly 64 hexadecimal characters")
    );
}

#[test]
fn preview_projects_every_operation_without_executor_handles_or_full_images() {
    let update_before = observed_file(b"update-before-secret", 1);
    let update_after = b"update-after-secret";
    let delete_before = observed_file(b"delete-before-secret", 2);
    let move_before = observed_file(b"move-before-secret", 3);
    let move_after = b"move-after-secret";
    let plan = planned_transaction(vec![
        PlannedMutation::Delete {
            path: "native:z-delete".to_string(),
            model_path: "z-delete".to_string(),
            path_key: path_key("z-delete"),
            before: delete_before.clone(),
        },
        PlannedMutation::Move {
            source: "native:m-source".to_string(),
            model_source: "m-source".to_string(),
            source_key: path_key("m-source"),
            before: move_before.clone(),
            destination: "native:n-destination".to_string(),
            model_destination: "n-destination".to_string(),
            destination_key: path_key("n-destination"),
            contents: move_after.to_vec(),
            after_digest: ExactBytesDigest::new(move_after),
        },
        PlannedMutation::Update {
            path: "native:c-update".to_string(),
            model_path: "c-update".to_string(),
            path_key: path_key("c-update"),
            before: update_before.clone(),
            contents: update_after.to_vec(),
            after_digest: ExactBytesDigest::new(update_after),
        },
        create("a-create", b"create-after-secret"),
    ]);
    let limits = TransactionLimits {
        max_preview_bytes: 16,
        ..TransactionLimits::default()
    };

    let preview = build_preview(&plan, limits).unwrap();

    assert_eq!(
        preview.mutations,
        vec![
            MutationPreview::Create {
                path: "a-create".to_string(),
                after_digest: ExactBytesDigest::new(b"create-after-secret"),
                content: PreviewText {
                    text: "create-after-sec".to_string(),
                    truncated: true,
                },
            },
            MutationPreview::Update {
                path: "c-update".to_string(),
                before_digest: update_before.exact_digest,
                after_digest: ExactBytesDigest::new(update_after),
                content: PreviewText {
                    text: String::new(),
                    truncated: true,
                },
            },
            MutationPreview::Move {
                source: "m-source".to_string(),
                destination: "n-destination".to_string(),
                before_digest: move_before.exact_digest,
                after_digest: ExactBytesDigest::new(move_after),
                content: PreviewText {
                    text: String::new(),
                    truncated: true,
                },
            },
            MutationPreview::Delete {
                path: "z-delete".to_string(),
                before_digest: delete_before.exact_digest,
                content: PreviewText {
                    text: String::new(),
                    truncated: true,
                },
            },
        ]
    );
    assert_eq!(preview.preview_bytes, 16);
    assert!(preview.preview_truncated);

    let serialized = serde_json::to_string(&preview).unwrap();
    let decoded = serde_json::from_str::<PlanPreview>(&serialized).unwrap();
    assert_eq!(decoded, preview);
    assert!(!serialized.contains("native:"));
    assert!(!serialized.contains("opened-root-handle"));
    assert!(!serialized.contains("after-secret"));
    assert!(!serialized.contains("before-secret"));
    let value = serde_json::to_value(&preview).unwrap();
    let mutations = value["mutations"].as_array().unwrap();
    for mutation in mutations {
        for field in ["before_digest", "after_digest"] {
            if let Some(digest) = mutation.get(field) {
                assert_eq!(digest.as_str().unwrap().len(), 64);
            }
        }
        assert!(mutation.get("beforeDigest").is_none());
        assert!(mutation.get("afterDigest").is_none());
    }
    assert_eq!(
        mutations
            .iter()
            .filter(|mutation| mutation.get("before_digest").is_some())
            .count(),
        3
    );
    assert_eq!(
        mutations
            .iter()
            .filter(|mutation| mutation.get("after_digest").is_some())
            .count(),
        3
    );
}

#[test]
fn preview_budget_is_utf8_safe_and_independent_of_request_order() {
    let first = create("a", "éé".as_bytes());
    let second = create("b", b"abc");
    let forward = planned_transaction(vec![first.clone(), second.clone()]);
    let reversed = planned_transaction(vec![second, first]);
    let limits = TransactionLimits {
        max_preview_bytes: 3,
        ..TransactionLimits::default()
    };

    let forward_preview = build_preview(&forward, limits).unwrap();
    let reversed_preview = build_preview(&reversed, limits).unwrap();

    assert_eq!(forward_preview, reversed_preview);
    assert_eq!(forward_preview.preview_bytes, 3);
    assert_eq!(
        forward_preview.mutations,
        vec![
            MutationPreview::Create {
                path: "a".to_string(),
                after_digest: ExactBytesDigest::new("éé".as_bytes()),
                content: PreviewText {
                    text: "é".to_string(),
                    truncated: true,
                },
            },
            MutationPreview::Create {
                path: "b".to_string(),
                after_digest: ExactBytesDigest::new(b"abc"),
                content: PreviewText {
                    text: "a".to_string(),
                    truncated: true,
                },
            },
        ]
    );
}

#[test]
fn response_budget_truncates_json_escape_expansion() {
    let contents = [b'\x01', b'\x01'];
    let plan = planned_transaction(vec![create("a", &contents)]);
    let skeleton_limits = TransactionLimits {
        max_preview_bytes: 0,
        ..TransactionLimits::default()
    };
    let skeleton = build_preview(&plan, skeleton_limits).unwrap();
    let skeleton_bytes = serde_json::to_vec(&skeleton).unwrap().len() as u64;
    let limits = TransactionLimits {
        max_preview_bytes: contents.len() as u64,
        max_response_bytes: skeleton_bytes + 6,
        ..TransactionLimits::default()
    };

    let preview = build_preview(&plan, limits).unwrap();

    assert_eq!(preview.preview_bytes, 1);
    assert!(preview.preview_truncated);
    assert_eq!(
        preview.mutations,
        vec![MutationPreview::Create {
            path: "a".to_string(),
            after_digest: ExactBytesDigest::new(&contents),
            content: PreviewText {
                text: "\u{1}".to_string(),
                truncated: true,
            },
        }]
    );
    assert_eq!(
        serde_json::to_vec(&preview).unwrap().len() as u64,
        skeleton_bytes + 6
    );
}

#[test]
fn preview_rejects_response_limit_smaller_than_the_structural_summary() {
    let plan = planned_transaction(vec![create("a", b"contents")]);
    let skeleton_limits = TransactionLimits {
        max_preview_bytes: 0,
        ..TransactionLimits::default()
    };
    let skeleton = build_preview(&plan, skeleton_limits).unwrap();
    let serialized_len = serde_json::to_vec(&skeleton).unwrap().len() as u64;
    let limits = TransactionLimits {
        max_preview_bytes: 0,
        max_response_bytes: serialized_len - 1,
        ..TransactionLimits::default()
    };

    let error = build_preview(&plan, limits).unwrap_err();

    assert_eq!(
        error,
        PlanError::Limit {
            resource: "response bytes",
            observed: serialized_len,
            limit: serialized_len - 1,
        }
    );
}
