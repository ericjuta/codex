use std::collections::BTreeMap;

use codex_utils_path_uri::PathUri;
use futures::executor::block_on;
use pretty_assertions::assert_eq;

use super::executor_test_support::TestFileSystem;
use super::*;

#[test]
fn immediate_commit_plans_and_executes_in_one_call() {
    let file_system = TestFileSystem::new([]);
    let request = create_request(TransactionAction::Commit);
    let expected_plan = block_on(plan(&file_system, request.clone())).unwrap();
    let transaction_id = TransactionId("tx-immediate".to_string());

    let result = block_on(execute_request(
        &file_system,
        request,
        transaction_id.clone(),
        TransactionLimits::default(),
    ))
    .unwrap();

    assert_eq!(
        result,
        ExecutionResult {
            transaction_id,
            plan_digest: expected_plan.plan_digest,
            outcome: ExecutionOutcome::Committed,
        }
    );
    assert_eq!(
        file_system.files(),
        BTreeMap::from([("created.txt".to_string(), b"created\n".to_vec())])
    );
}

#[test]
fn matching_preview_digest_replans_and_commits() {
    let file_system = TestFileSystem::new([]);
    let preview = block_on(plan(
        &file_system,
        create_request(TransactionAction::Preview),
    ))
    .unwrap();
    let transaction_id = TransactionId("tx-previewed".to_string());

    let result = block_on(execute_request(
        &file_system,
        create_request(TransactionAction::CommitPreviewed {
            expected_plan_digest: preview.plan_digest,
        }),
        transaction_id.clone(),
        TransactionLimits::default(),
    ))
    .unwrap();

    assert_eq!(
        result,
        ExecutionResult {
            transaction_id,
            plan_digest: preview.plan_digest,
            outcome: ExecutionOutcome::Committed,
        }
    );
}

#[test]
fn mismatched_preview_digest_fails_before_storage_or_mutation() {
    let file_system = TestFileSystem::new([]);
    let preview = block_on(plan(
        &file_system,
        create_request(TransactionAction::Preview),
    ))
    .unwrap();
    let expected_plan_digest = ExactBytesDigest::from_array([0x5a; 32]);

    let error = block_on(execute_request(
        &file_system,
        create_request(TransactionAction::CommitPreviewed {
            expected_plan_digest,
        }),
        TransactionId("tx-mismatch".to_string()),
        TransactionLimits::default(),
    ))
    .unwrap_err();

    assert_eq!(
        error,
        ExecuteRequestError::Plan(PlanError::PlanDigestMismatch {
            expected: expected_plan_digest,
            actual: preview.plan_digest,
        })
    );
    assert_eq!(file_system.files(), BTreeMap::new());
    assert_eq!(file_system.journals(), Vec::new());
    assert_eq!(file_system.artifact_counts(), (0, 0));
    assert_eq!(file_system.events(), Vec::new());
}

#[test]
fn preview_action_is_rejected_without_planning() {
    let file_system = TestFileSystem::new([]);

    let error = block_on(execute_request(
        &file_system,
        create_request(TransactionAction::Preview),
        TransactionId("tx-preview".to_string()),
        TransactionLimits::default(),
    ))
    .unwrap_err();

    assert_eq!(error, ExecuteRequestError::PreviewAction);
    assert_eq!(file_system.events(), Vec::new());
}

fn create_request(action: TransactionAction) -> TransactionRequest {
    TransactionRequest {
        environment_id: "test-environment".to_string(),
        root: PathUri::parse("file:///workspace").unwrap(),
        action,
        mutations: vec![FileMutation::Create {
            path: "created.txt".to_string(),
            contents: b"created\n".to_vec(),
        }],
    }
}
