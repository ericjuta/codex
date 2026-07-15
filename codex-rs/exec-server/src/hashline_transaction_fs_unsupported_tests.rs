use codex_hashline_transaction::PlanningFileSystem;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;

use crate::hashline_transaction_fs::NativePlanningFileSystem;
use crate::hashline_transaction_fs::NativeTransactionFileSystem;

#[tokio::test]
async fn unsupported_platform_fails_before_planning() {
    let root = AbsolutePathBuf::current_dir().expect("current directory should be absolute");
    let root = PathUri::from_abs_path(&root);

    assert_eq!(
        NativePlanningFileSystem
            .open_root(&root)
            .await
            .expect_err("unsupported platform must fail"),
        TransactionFileSystemError::Unsupported {
            capability: "hashline transaction planning",
            reason: "native no-follow path handles are not implemented on this executor platform"
                .to_string(),
        }
    );
}

#[tokio::test]
async fn transaction_filesystem_remains_root_bound_and_unsupported() {
    let root = AbsolutePathBuf::current_dir().expect("current directory should be absolute");
    let root = PathUri::from_abs_path(&root);
    let different_root = PathUri::parse("file:///different-root").expect("parse different root");
    let file_system =
        NativeTransactionFileSystem::new("unsupported-test-environment".to_string(), root.clone());

    assert_eq!(
        file_system
            .open_root(&different_root)
            .await
            .expect_err("different root must fail first"),
        TransactionFileSystemError::InvalidRoot {
            root: different_root,
            reason: format!("transaction filesystem is configured for root `{root}`"),
        }
    );
    assert_eq!(
        file_system
            .open_root(&root)
            .await
            .expect_err("configured root remains unsupported"),
        TransactionFileSystemError::Unsupported {
            capability: "hashline transaction planning",
            reason: "native no-follow path handles are not implemented on this executor platform"
                .to_string(),
        }
    );
}
