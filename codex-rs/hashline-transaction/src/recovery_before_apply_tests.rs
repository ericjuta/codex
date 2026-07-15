use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;

use codex_utils_path_uri::PathUri;
use futures::executor::block_on;
use pretty_assertions::assert_eq;

use super::executor_test_support::TestFileSystem;
use super::*;

#[test]
fn untouched_original_converges_from_pre_apply_update_delete_and_move() {
    let cases = [
        (
            "update",
            FileMutation::Update {
                path: "source".to_string(),
                expected: expected(),
                edits: vec![FileEdit::ReplaceAll {
                    contents: b"new".to_vec(),
                }],
            },
        ),
        (
            "delete",
            FileMutation::Delete {
                path: "source".to_string(),
                expected: expected(),
            },
        ),
        (
            "move",
            FileMutation::Move {
                source: "source".to_string(),
                expected: expected(),
                destination: "destination".to_string(),
                edits: Vec::new(),
            },
        ),
    ];

    for (name, mutation) in cases {
        let file_system = TestFileSystem::new([("source", b"old".as_slice(), 1)]);
        let plan = block_on(plan(&file_system, request(mutation))).unwrap();
        let plan_digest = plan.plan_digest;
        let transaction_id = format!("tx-before-{name}");
        file_system.crash_after_persist_at(/*call*/ 4);

        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                let _ = block_on(execute(
                    &file_system,
                    plan,
                    TransactionId(transaction_id.clone()),
                    TransactionLimits::default(),
                ));
            }))
            .is_err()
        );
        assert_eq!(
            file_system.journals().last().unwrap().mutations[0].progress,
            MutationProgress::Committing,
            "{name}"
        );
        assert_eq!(file_system.files(), original_files(), "{name}");

        assert_eq!(
            block_on(recover_transaction(
                &file_system.restart(),
                &TestFileSystem::transaction_key(&transaction_id),
                TransactionLimits::default(),
            ))
            .unwrap(),
            RecoveryResult {
                transaction_id: TransactionId(transaction_id),
                plan_digest,
                outcome: RecoveryOutcome::RolledBack,
            },
            "{name}"
        );
        assert_eq!(file_system.files(), original_files(), "{name}");
        assert_eq!(file_system.artifact_counts(), (0, 0), "{name}");
    }
}

fn request(mutation: FileMutation) -> TransactionRequest {
    TransactionRequest {
        environment_id: "test-environment".to_string(),
        root: PathUri::parse("file:///workspace").unwrap(),
        action: TransactionAction::Commit,
        mutations: vec![mutation],
    }
}

fn expected() -> ExpectedFile {
    ExpectedFile {
        exact_digest: ExactBytesDigest::new(b"old"),
    }
}

fn original_files() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([("source".to_string(), b"old".to_vec())])
}
