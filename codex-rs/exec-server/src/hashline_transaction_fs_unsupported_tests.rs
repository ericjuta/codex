use codex_hashline_transaction::PlanningFileSystem;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;

use crate::hashline_transaction_fs::NativePlanningFileSystem;

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
