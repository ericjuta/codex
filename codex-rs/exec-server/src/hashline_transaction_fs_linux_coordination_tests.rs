use std::fs;
use std::sync::Arc;
use std::sync::Barrier;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use codex_hashline_transaction::ObservationLimit;
use codex_hashline_transaction::ObservedPath;
use codex_hashline_transaction::PlanningFileSystem;
use codex_hashline_transaction::TransactionCoordination;
use codex_hashline_transaction::TransactionFileSystemError;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::collect_lock_targets;
use crate::hashline_transaction_fs::NativeTransactionFileSystem;

fn root_uri(temp: &TempDir) -> PathUri {
    let root = AbsolutePathBuf::from_absolute_path_checked(temp.path())
        .unwrap_or_else(|error| panic!("temporary directory should be absolute: {error}"));
    PathUri::from_abs_path(&root)
}

fn file_system(root: PathUri) -> NativeTransactionFileSystem {
    NativeTransactionFileSystem::new("local-test-environment".to_string(), root)
}

#[tokio::test]
async fn transaction_filesystem_opens_only_its_configured_root() {
    let configured = tempfile::tempdir().expect("create configured root");
    let other = tempfile::tempdir().expect("create other root");
    let configured_uri = root_uri(&configured);
    let other_uri = root_uri(&other);
    let file_system = file_system(configured_uri.clone());

    assert_eq!(file_system.environment_id(), "local-test-environment");
    file_system
        .open_root(&configured_uri)
        .await
        .expect("open configured root");
    assert_eq!(
        file_system
            .open_root(&other_uri)
            .await
            .expect_err("different root must be rejected"),
        TransactionFileSystemError::InvalidRoot {
            root: other_uri,
            reason: format!("transaction filesystem is configured for root `{configured_uri}`"),
        }
    );
}

#[tokio::test]
async fn resolution_reserves_the_future_root_sidecar_name() {
    let temp = tempfile::tempdir().expect("create transaction root");
    let root_uri = root_uri(&temp);
    let file_system = file_system(root_uri.clone());
    let root = file_system.open_root(&root_uri).await.expect("open root");

    assert_eq!(
        file_system
            .resolve(&root, ".codex-hashline-transactions/journal")
            .await
            .expect_err("sidecar path must be reserved"),
        TransactionFileSystemError::InvalidModelPath {
            path: ".codex-hashline-transactions/journal".to_string(),
            reason: "path uses the reserved Hashline transaction sidecar name".to_string(),
        }
    );
}

#[tokio::test]
async fn lock_targets_are_deduplicated_and_canonically_ordered() {
    let temp = tempfile::tempdir().expect("create transaction root");
    fs::create_dir_all(temp.path().join("a")).expect("create first parent");
    fs::create_dir_all(temp.path().join("b")).expect("create second parent");
    fs::write(temp.path().join("a/one"), b"one").expect("write first fixture");
    fs::write(temp.path().join("a/two"), b"two").expect("write second fixture");
    fs::write(temp.path().join("b/three"), b"three").expect("write third fixture");
    let root_uri = root_uri(&temp);
    let file_system = file_system(root_uri.clone());
    let root = file_system.open_root(&root_uri).await.expect("open root");
    let b = file_system
        .resolve(&root, "b/three")
        .await
        .expect("resolve b");
    let a_one = file_system
        .resolve(&root, "a/one")
        .await
        .expect("resolve a/one");
    let a_two = file_system
        .resolve(&root, "a/two")
        .await
        .expect("resolve a/two");

    let (targets, covered_paths) = collect_lock_targets(
        &root,
        &[b.clone(), a_one.clone(), a_two.clone(), a_one.clone()],
    )
    .expect("collect lock targets");
    let identities = targets
        .iter()
        .map(|target| target.identity)
        .collect::<Vec<_>>();
    let mut sorted = identities.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(identities, sorted);
    assert_eq!(targets.len(), 2);
    assert_eq!(covered_paths.len(), 3);

    let lease = tokio::time::timeout(
        Duration::from_secs(/*secs*/ 2),
        file_system.lock_paths(&root, &[a_one, a_two]),
    )
    .await
    .expect("same-parent lock acquisition must not self-deadlock")
    .expect("lock same-parent paths");
    assert_eq!(lease._locked_directories.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_instances_serialize_on_the_same_parent_directory() {
    let temp = tempfile::tempdir().expect("create transaction root");
    fs::write(temp.path().join("file.txt"), b"contents").expect("write fixture");
    let root_uri = root_uri(&temp);
    let first_file_system = file_system(root_uri.clone());
    let first_root = first_file_system
        .open_root(&root_uri)
        .await
        .expect("open first root handle");
    let first_path = first_file_system
        .resolve(&first_root, "file.txt")
        .await
        .expect("resolve first path");
    let first_lease = first_file_system
        .lock_paths(&first_root, &[first_path])
        .await
        .expect("acquire first lease");

    let barrier = Arc::new(Barrier::new(/*n*/ 2));
    let worker_barrier = Arc::clone(&barrier);
    let (acquired_tx, acquired_rx) = mpsc::sync_channel(/*bound*/ 1);
    let worker = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create worker runtime");
        let result = runtime.block_on(async move {
            let second_file_system = file_system(root_uri.clone());
            let second_root = second_file_system
                .open_root(&root_uri)
                .await
                .map_err(|error| error.to_string())?;
            let second_path = second_file_system
                .resolve(&second_root, "file.txt")
                .await
                .map_err(|error| error.to_string())?;
            worker_barrier.wait();
            let lease = second_file_system
                .lock_paths(&second_root, &[second_path])
                .await
                .map_err(|error| error.to_string())?;
            drop(lease);
            Ok::<(), String>(())
        });
        acquired_tx.send(result).expect("report worker lock result");
    });

    barrier.wait();
    assert!(
        matches!(
            acquired_rx.recv_timeout(Duration::from_millis(/*millis*/ 250)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "second instance must remain blocked while the first lease is retained"
    );
    drop(first_lease);
    assert_eq!(
        acquired_rx
            .recv_timeout(Duration::from_secs(/*secs*/ 5))
            .expect("second instance should acquire after release"),
        Ok(())
    );
    worker.join().expect("join lock worker");
}

#[tokio::test]
async fn locked_reobservation_requires_lease_coverage_and_detects_replacement() {
    let temp = tempfile::tempdir().expect("create transaction root");
    fs::write(temp.path().join("covered.txt"), b"same bytes").expect("write covered fixture");
    fs::write(temp.path().join("outside.txt"), b"outside").expect("write outside fixture");
    let root_uri = root_uri(&temp);
    let file_system = file_system(root_uri.clone());
    let root = file_system.open_root(&root_uri).await.expect("open root");
    let covered = file_system
        .resolve(&root, "covered.txt")
        .await
        .expect("resolve covered path");
    let outside = file_system
        .resolve(&root, "outside.txt")
        .await
        .expect("resolve uncovered path");
    let lease = file_system
        .lock_paths(&root, std::slice::from_ref(&covered))
        .await
        .expect("lock covered path");

    assert!(matches!(
        file_system
            .reobserve_locked(&lease, &covered, ObservationLimit { max_bytes: 1024 })
            .await
            .expect("reobserve covered path"),
        ObservedPath::Present(_)
    ));
    assert_eq!(
        file_system
            .reobserve_locked(&lease, &outside, ObservationLimit { max_bytes: 1024 })
            .await
            .expect_err("uncovered path must be rejected"),
        TransactionFileSystemError::Platform {
            operation: "reobserve locked path",
            reason: "path `outside.txt` is not covered by the retained transaction lease"
                .to_string(),
        }
    );

    fs::rename(
        temp.path().join("covered.txt"),
        temp.path().join("original.txt"),
    )
    .expect("move original aside");
    fs::write(temp.path().join("covered.txt"), b"same bytes").expect("write replacement");
    assert_eq!(
        file_system
            .reobserve_locked(&lease, &covered, ObservationLimit { max_bytes: 1024 })
            .await
            .expect_err("identical-byte replacement must be stale"),
        TransactionFileSystemError::ChangedSincePlanning {
            path: "covered.txt".to_string(),
        }
    );
}
