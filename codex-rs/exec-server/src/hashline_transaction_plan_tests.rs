#![cfg(target_os = "linux")]

use std::fs;

use codex_exec_server_protocol::HASHLINE_TRANSACTION_CONFLICT_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_MAX_RESPONSE_BYTES;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_RPC_RESPONSE_OVERHEAD_BYTES;
use codex_exec_server_protocol::HASHLINE_TRANSACTION_UNSUPPORTED_ERROR_CODE;
use codex_exec_server_protocol::JSONRPCMessage;
use codex_exec_server_protocol::JSONRPCResponse;
use codex_exec_server_protocol::RequestId;
use codex_hashline_transaction::ExactBytesDigest;
use codex_hashline_transaction::FileMutation;
use codex_hashline_transaction::MutationPreview;
use codex_hashline_transaction::PlanError;
use codex_hashline_transaction::PlanSummary;
use codex_hashline_transaction::PreviewText;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_protocol::protocol::SandboxPolicy;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;

use super::map_plan_error;
use super::plan_direct;
use super::plan_rpc_error;
use crate::Environment;
use crate::FileSystemSandboxContext;
use crate::HashlineTransactionPlanRequest;
use crate::LOCAL_ENVIRONMENT_ID;
use crate::protocol::HashlineTransactionPlanParams;

fn root_uri(temp: &tempfile::TempDir) -> PathUri {
    let root = AbsolutePathBuf::from_absolute_path_checked(temp.path())
        .unwrap_or_else(|error| panic!("temporary directory should be absolute: {error}"));
    PathUri::from_abs_path(&root)
}

fn create_request(root: PathUri) -> HashlineTransactionPlanRequest {
    HashlineTransactionPlanRequest {
        root,
        mutations: vec![FileMutation::Create {
            path: "new.txt".to_string(),
            contents: b"planned contents".to_vec(),
        }],
        sandbox: None,
    }
}

fn create_params(root: PathUri) -> HashlineTransactionPlanParams {
    let request = create_request(root);
    HashlineTransactionPlanParams {
        environment_id: "test-env".to_string(),
        root: request.root,
        mutations: request.mutations,
        sandbox: request.sandbox,
    }
}

#[tokio::test]
async fn direct_planning_returns_a_deterministic_bounded_preview_without_writing() {
    let temp = tempfile::tempdir().expect("create temporary directory");
    let request = create_request(root_uri(&temp));
    let environment = Environment::default_for_tests();

    let first = environment
        .plan_hashline_transaction(request.clone())
        .await
        .expect("first plan");
    let second = environment
        .plan_hashline_transaction(request)
        .await
        .expect("second plan");

    assert_eq!(second, first);
    assert_eq!(first.preview.environment_id, LOCAL_ENVIRONMENT_ID);
    assert_eq!(
        first.preview.summary,
        PlanSummary {
            creates: 1,
            after_bytes: 16,
            ..PlanSummary::default()
        }
    );
    assert_eq!(
        first.preview.mutations,
        vec![MutationPreview::Create {
            path: "new.txt".to_string(),
            after_digest: ExactBytesDigest::new(b"planned contents"),
            content: PreviewText {
                text: "planned contents".to_string(),
                truncated: false,
            },
        }]
    );
    let preview_json = serde_json::to_vec(&first.preview).expect("serialize preview");
    let wire_json = serde_json::to_vec(&JSONRPCMessage::Response(JSONRPCResponse {
        id: RequestId::Integer(i64::MIN),
        result: serde_json::to_value(&first).expect("serialize response result"),
    }))
    .expect("serialize wire response");
    assert!(
        wire_json.len().saturating_sub(preview_json.len()) as u64
            <= HASHLINE_TRANSACTION_RPC_RESPONSE_OVERHEAD_BYTES
    );
    assert!(wire_json.len() as u64 <= HASHLINE_TRANSACTION_MAX_RESPONSE_BYTES);
    assert!(!temp.path().join("new.txt").exists());
}

#[tokio::test]
async fn direct_planning_rejects_a_restricted_sandbox_context() {
    let temp = tempfile::tempdir().expect("create temporary directory");
    fs::write(temp.path().join("existing.txt"), b"unchanged").expect("write fixture");
    let root = root_uri(&temp);
    let mut params = create_params(root.clone());
    params.sandbox = Some(
        FileSystemSandboxContext::from_legacy_sandbox_policy(
            SandboxPolicy::ReadOnly {
                network_access: false,
            },
            root,
        )
        .expect("sandbox context"),
    );

    let error = plan_direct(params)
        .await
        .expect_err("restricted planning must use the sandbox runner");

    assert_eq!(error.code, HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.message,
        "restricted Hashline transaction planning must run in the filesystem sandbox"
    );
    assert_eq!(
        fs::read(temp.path().join("existing.txt")).unwrap(),
        b"unchanged"
    );
    assert!(!temp.path().join("new.txt").exists());
}

#[test]
fn planning_errors_have_stable_categories() {
    let errors = [
        PlanError::Empty,
        PlanError::FileSystem(TransactionFileSystemError::Unsupported {
            capability: "planning",
            reason: "not available".to_string(),
        }),
        PlanError::FileSystem(TransactionFileSystemError::ChangedDuringPlanning {
            path: "stale.txt".to_string(),
        }),
        PlanError::FileSystem(TransactionFileSystemError::Platform {
            operation: "open",
            reason: "I/O failure".to_string(),
        }),
    ];

    assert_eq!(
        errors.map(|error| map_plan_error(error).code),
        [
            HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE,
            HASHLINE_TRANSACTION_UNSUPPORTED_ERROR_CODE,
            HASHLINE_TRANSACTION_CONFLICT_ERROR_CODE,
            HASHLINE_TRANSACTION_EXECUTOR_ERROR_CODE,
        ]
    );
}

#[test]
fn planning_error_messages_are_utf8_safe_and_bounded() {
    let error = plan_rpc_error(
        HASHLINE_TRANSACTION_INVALID_REQUEST_ERROR_CODE,
        "é".repeat(HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES),
    );

    assert!(error.message.len() <= HASHLINE_TRANSACTION_MAX_ERROR_MESSAGE_BYTES);
    assert!(error.message.ends_with("..."));
}
