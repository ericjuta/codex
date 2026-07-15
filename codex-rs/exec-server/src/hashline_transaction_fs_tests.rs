#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;

use codex_hashline_transaction::ExactBytesDigest;
use codex_hashline_transaction::FileKind;
use codex_hashline_transaction::ObservationLimit;
use codex_hashline_transaction::ObservedPath;
use codex_hashline_transaction::PlanningFileSystem;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use crate::hashline_transaction_fs::NativePlanningFileSystem;

fn root_uri(temp: &TempDir) -> PathUri {
    let root = AbsolutePathBuf::from_absolute_path_checked(temp.path())
        .unwrap_or_else(|error| panic!("temporary directory should be absolute: {error}"));
    PathUri::from_abs_path(&root)
}

fn observation_limit(max_bytes: u64) -> ObservationLimit {
    ObservationLimit { max_bytes }
}

#[tokio::test]
async fn observes_exact_bytes_and_stable_executor_evidence() {
    let temp = tempfile::tempdir().expect("create temporary directory");
    fs::create_dir(temp.path().join("nested")).expect("create nested directory");
    let contents = b"first\r\nsecond\n\0";
    fs::write(temp.path().join("nested/file.txt"), contents).expect("write fixture");
    let file_system = NativePlanningFileSystem;
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open root");
    let path = file_system
        .resolve(&root, "nested/./file.txt")
        .await
        .expect("resolve file");
    let alias = file_system
        .resolve(&root, "nested//file.txt")
        .await
        .expect("resolve alias");

    assert_eq!(path, alias);
    assert_eq!(
        file_system
            .canonical_path_key(&path)
            .expect("derive canonical key"),
        file_system
            .canonical_path_key(&alias)
            .expect("derive alias key")
    );
    let first = file_system
        .observe(&path, observation_limit(1024))
        .await
        .expect("observe file");
    let second = file_system
        .observe(&path, observation_limit(1024))
        .await
        .expect("observe file again");
    assert_eq!(first, second);
    let ObservedPath::Present(observed) = first else {
        panic!("fixture file should be present");
    };
    assert_eq!(observed.contents, contents);
    assert_eq!(observed.exact_digest, ExactBytesDigest::new(contents));
    assert_eq!(observed.kind, FileKind::File);
    assert_eq!(observed.link_count.get(), 1);
    assert_eq!(observed.identity.namespace, "linux-dev-ino-v1");
    assert_eq!(observed.metadata.namespace, "linux-basic-restorable-v1");
}

#[tokio::test]
async fn resolves_an_absent_leaf_without_exposing_mutation() {
    let temp = tempfile::tempdir().expect("create temporary directory");
    let file_system = NativePlanningFileSystem;
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open root");
    let path = file_system
        .resolve(&root, "new.txt")
        .await
        .expect("resolve absent leaf");

    assert_eq!(
        file_system
            .observe(&path, observation_limit(1024))
            .await
            .expect("observe absent leaf"),
        ObservedPath::Absent
    );
}

#[tokio::test]
async fn rejects_paths_that_escape_or_name_the_root() {
    let temp = tempfile::tempdir().expect("create temporary directory");
    let file_system = NativePlanningFileSystem;
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open root");

    assert_eq!(
        file_system
            .resolve(&root, "../outside")
            .await
            .expect_err("parent traversal must fail"),
        TransactionFileSystemError::InvalidModelPath {
            path: "../outside".to_string(),
            reason: "parent traversal is not allowed".to_string(),
        }
    );
    assert_eq!(
        file_system
            .resolve(&root, ".")
            .await
            .expect_err("root path must fail"),
        TransactionFileSystemError::InvalidModelPath {
            path: ".".to_string(),
            reason: "path resolves to the selected root".to_string(),
        }
    );
    assert_eq!(
        file_system
            .resolve(&root, "/outside")
            .await
            .expect_err("absolute escape must fail"),
        TransactionFileSystemError::InvalidModelPath {
            path: "/outside".to_string(),
            reason: "absolute path is outside the selected root".to_string(),
        }
    );
    assert_eq!(
        file_system
            .resolve(&root, "bad\0path")
            .await
            .expect_err("NUL must fail"),
        TransactionFileSystemError::InvalidModelPath {
            path: "bad\0path".to_string(),
            reason: "path contains NUL".to_string(),
        }
    );
}

#[tokio::test]
async fn rejects_foreign_root_path_conventions() {
    let foreign = PathUri::parse("file:///C:/Windows").expect("parse Windows file URI");
    let error = NativePlanningFileSystem
        .open_root(&foreign)
        .await
        .expect_err("foreign root must fail closed");

    assert!(matches!(
        error,
        TransactionFileSystemError::InvalidRoot { root, .. } if root == foreign
    ));
}

#[tokio::test]
async fn rejects_final_and_intermediate_symbolic_links() {
    let temp = tempfile::tempdir().expect("create temporary directory");
    fs::create_dir(temp.path().join("real")).expect("create real directory");
    fs::write(temp.path().join("real/file.txt"), b"bytes").expect("write fixture");
    symlink("real/file.txt", temp.path().join("file-link")).expect("create file symlink");
    symlink("real", temp.path().join("dir-link")).expect("create directory symlink");
    let file_system = NativePlanningFileSystem;
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open root");

    assert_eq!(
        file_system
            .resolve(&root, "file-link")
            .await
            .expect_err("final symlink must fail"),
        TransactionFileSystemError::SymbolicLink {
            path: "file-link".to_string(),
        }
    );
    assert_eq!(
        file_system
            .resolve(&root, "dir-link/file.txt")
            .await
            .expect_err("intermediate symlink must fail"),
        TransactionFileSystemError::SymbolicLink {
            path: "dir-link/file.txt".to_string(),
        }
    );
}

#[tokio::test]
async fn rejects_a_symbolic_link_in_the_opened_root() {
    let parent = tempfile::tempdir().expect("create temporary directory");
    fs::create_dir(parent.path().join("real")).expect("create real root");
    symlink("real", parent.path().join("linked")).expect("create root symlink");
    let linked = AbsolutePathBuf::from_absolute_path_checked(parent.path().join("linked"))
        .expect("linked root should be absolute");
    let root_uri = PathUri::from_abs_path(&linked);

    assert_eq!(
        NativePlanningFileSystem
            .open_root(&root_uri)
            .await
            .expect_err("root symlink must fail"),
        TransactionFileSystemError::SymbolicLink {
            path: root_uri.to_string(),
        }
    );
}

#[tokio::test]
async fn enforces_the_observation_limit_before_retaining_contents() {
    let temp = tempfile::tempdir().expect("create temporary directory");
    fs::write(temp.path().join("large.txt"), b"four").expect("write fixture");
    let file_system = NativePlanningFileSystem;
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open root");
    let path = file_system
        .resolve(&root, "large.txt")
        .await
        .expect("resolve file");

    assert_eq!(
        file_system
            .observe(&path, observation_limit(3))
            .await
            .expect_err("oversized observation must fail"),
        TransactionFileSystemError::Platform {
            operation: "observe path",
            reason: "file requires at least 4 bytes, exceeding limit 3".to_string(),
        }
    );
}

#[tokio::test]
async fn rejects_filesystems_without_proven_path_key_semantics() {
    let proc_root =
        AbsolutePathBuf::from_absolute_path_checked("/proc").expect("proc path should be absolute");
    let error = NativePlanningFileSystem
        .open_root(&PathUri::from_abs_path(&proc_root))
        .await
        .expect_err("procfs must fail capability negotiation");

    assert!(matches!(
        error,
        TransactionFileSystemError::Unsupported {
            capability: "byte-exact transaction path keys",
            ..
        }
    ));
}

#[tokio::test]
async fn reports_directory_kind_and_hard_link_topology() {
    let temp = tempfile::tempdir().expect("create temporary directory");
    fs::create_dir(temp.path().join("directory")).expect("create directory");
    fs::write(temp.path().join("original"), b"bytes").expect("write fixture");
    fs::hard_link(temp.path().join("original"), temp.path().join("alias"))
        .expect("create hard link");
    let file_system = NativePlanningFileSystem;
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open root");
    let directory = file_system
        .resolve(&root, "directory")
        .await
        .expect("resolve directory");
    let hard_link = file_system
        .resolve(&root, "original")
        .await
        .expect("resolve hard-linked file");

    let ObservedPath::Present(directory) = file_system
        .observe(&directory, observation_limit(0))
        .await
        .expect("observe directory")
    else {
        panic!("directory should be present");
    };
    assert_eq!(directory.kind, FileKind::Directory);
    assert_eq!(directory.contents, Vec::<u8>::new());
    let ObservedPath::Present(hard_link) = file_system
        .observe(&hard_link, observation_limit(1024))
        .await
        .expect("observe hard link")
    else {
        panic!("hard link should be present");
    };
    assert_eq!(hard_link.link_count.get(), 2);
}

#[tokio::test]
async fn detects_a_replaced_path_even_when_bytes_match() {
    let temp = tempfile::tempdir().expect("create temporary directory");
    let path = temp.path().join("file.txt");
    fs::write(&path, b"same bytes").expect("write original");
    let file_system = NativePlanningFileSystem;
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open root");
    let resolved = file_system
        .resolve(&root, "file.txt")
        .await
        .expect("resolve original");
    fs::rename(&path, temp.path().join("old.txt")).expect("move original aside");
    fs::write(&path, b"same bytes").expect("write replacement");

    let error = file_system
        .observe(&resolved, observation_limit(1024))
        .await
        .expect_err("identity replacement must fail");
    assert!(matches!(
        error,
        TransactionFileSystemError::Platform {
            operation: "observe path",
            reason,
        } if reason.contains("changed while transaction planning")
    ));
}

#[tokio::test]
async fn detects_a_parent_directory_moved_outside_the_root() {
    let temp = tempfile::tempdir().expect("create temporary directory");
    let root_path = temp.path().join("root");
    fs::create_dir_all(root_path.join("inside")).expect("create nested root");
    fs::write(root_path.join("inside/file.txt"), b"bytes").expect("write original");
    let root_path = AbsolutePathBuf::from_absolute_path_checked(&root_path)
        .expect("transaction root should be absolute");
    let file_system = NativePlanningFileSystem;
    let root = file_system
        .open_root(&PathUri::from_abs_path(&root_path))
        .await
        .expect("open root");
    let resolved = file_system
        .resolve(&root, "inside/file.txt")
        .await
        .expect("resolve original");
    fs::rename(
        root_path.as_path().join("inside"),
        temp.path().join("escaped"),
    )
    .expect("move parent outside root");

    let error = file_system
        .observe(&resolved, observation_limit(1024))
        .await
        .expect_err("detached parent must fail");
    assert!(matches!(
        error,
        TransactionFileSystemError::Platform {
            operation: "observe path",
            reason,
        } if reason.contains("changed while transaction planning")
    ));
}

#[tokio::test]
async fn detects_an_absent_leaf_that_appears_before_observation() {
    let temp = tempfile::tempdir().expect("create temporary directory");
    let file_system = NativePlanningFileSystem;
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open root");
    let resolved = file_system
        .resolve(&root, "new.txt")
        .await
        .expect("resolve absent leaf");
    fs::write(temp.path().join("new.txt"), b"appeared").expect("write concurrent file");

    let error = file_system
        .observe(&resolved, observation_limit(1024))
        .await
        .expect_err("appearing path must fail");
    assert!(matches!(
        error,
        TransactionFileSystemError::Platform {
            operation: "observe path",
            reason,
        } if reason.contains("changed while transaction planning")
    ));
}

#[tokio::test]
async fn metadata_fingerprint_changes_with_restorable_mode() {
    let temp = tempfile::tempdir().expect("create temporary directory");
    let path = temp.path().join("file.txt");
    fs::write(&path, b"bytes").expect("write fixture");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("set initial mode");
    let file_system = NativePlanningFileSystem;
    let root = file_system
        .open_root(&root_uri(&temp))
        .await
        .expect("open root");
    let resolved = file_system
        .resolve(&root, "file.txt")
        .await
        .expect("resolve file");
    let ObservedPath::Present(before) = file_system
        .observe(&resolved, observation_limit(1024))
        .await
        .expect("observe before chmod")
    else {
        panic!("file should be present");
    };
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("change mode");
    let ObservedPath::Present(after) = file_system
        .observe(&resolved, observation_limit(1024))
        .await
        .expect("observe after chmod")
    else {
        panic!("file should be present");
    };

    assert_ne!(before.metadata, after.metadata);
    assert_eq!(before.contents, after.contents);
    assert_eq!(before.identity, after.identity);
}
