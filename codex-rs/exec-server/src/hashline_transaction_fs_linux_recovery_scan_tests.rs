use std::fs;
use std::fs::File;

use super::*;

#[test]
fn directory_scan_sorts_entries_and_handles_end_of_directory() {
    let temp = tempfile::tempdir().expect("create scan directory");
    fs::create_dir(temp.path().join("b")).expect("create b");
    fs::create_dir(temp.path().join("a")).expect("create a");
    let directory = File::open(temp.path()).expect("open scan directory");

    assert_eq!(
        directory_names(&directory, /*max_entries*/ 2).expect("scan directory"),
        vec![OsString::from("a"), OsString::from("b")]
    );
}

#[test]
fn directory_scan_fails_closed_past_the_hard_bound() {
    let temp = tempfile::tempdir().expect("create scan directory");
    fs::create_dir(temp.path().join("a")).expect("create a");
    fs::create_dir(temp.path().join("b")).expect("create b");
    let directory = File::open(temp.path()).expect("open scan directory");

    let error = directory_names(&directory, /*max_entries*/ 1)
        .expect_err("scan must reject more entries than the bound");

    assert!(matches!(
        error,
        TransactionFileSystemError::Platform {
            operation: "enumerate transaction recovery storage",
            ..
        }
    ));
}
