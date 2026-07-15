use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;
use std::task::Context;
use std::task::Poll;

use codex_utils_path_uri::PathUri;
use futures::executor::block_on;
use futures::task::noop_waker;
use pretty_assertions::assert_eq;

use super::executor_test_support::TestFileSystem;
use super::*;

#[test]
fn recovery_storage_ownership_serializes_same_transaction() {
    let file_system = TestFileSystem::new([]);
    crash_pre_apply(&file_system, "tx-serial");
    let key = TestFileSystem::transaction_key("tx-serial");
    let first = block_on(file_system.lock_recovery_storage(&key)).unwrap();
    let mut waiting = Box::pin(file_system.lock_recovery_storage(&key));
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);

    assert!(matches!(waiting.as_mut().poll(&mut context), Poll::Pending));
    drop(first);
    let second = block_on(waiting).unwrap();
    assert_eq!(
        second.transaction_id,
        TransactionId("tx-serial".to_string())
    );
}

#[test]
fn duplicate_transaction_id_fails_before_new_storage_or_visible_mutation() {
    let file_system = TestFileSystem::new([]);
    let transaction_id = TransactionId("tx-duplicate".to_string());
    let first = block_on(plan(&file_system, request("first"))).unwrap();
    block_on(execute(
        &file_system,
        first,
        transaction_id.clone(),
        TransactionLimits::default(),
    ))
    .unwrap();
    let second = block_on(plan(&file_system, request("second"))).unwrap();

    assert_eq!(
        block_on(execute(
            &file_system,
            second,
            transaction_id,
            TransactionLimits::default(),
        )),
        Err(ExecuteError::BeforeCommit {
            failure: ExecutionFailure::FileSystem(TransactionFileSystemError::Platform {
                operation: "allocate storage",
                reason: "transaction ID already exists".to_string(),
            }),
        })
    );
    assert_eq!(
        file_system.files(),
        std::collections::BTreeMap::from([("first".to_string(), b"contents-first".to_vec())])
    );
}

fn crash_pre_apply(file_system: &TestFileSystem, transaction_id: &str) {
    let plan = block_on(plan(file_system, request("created"))).unwrap();
    file_system.crash_after_persist_at(/*call*/ 4);
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _ = block_on(execute(
                file_system,
                plan,
                TransactionId(transaction_id.to_string()),
                TransactionLimits::default(),
            ));
        }))
        .is_err()
    );
}

fn request(path: &str) -> TransactionRequest {
    TransactionRequest {
        environment_id: "test-environment".to_string(),
        root: PathUri::parse("file:///workspace").unwrap(),
        action: TransactionAction::Commit,
        mutations: vec![FileMutation::Create {
            path: path.to_string(),
            contents: format!("contents-{path}").into_bytes(),
        }],
    }
}
