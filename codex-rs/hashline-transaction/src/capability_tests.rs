use std::num::NonZeroU64;
use std::sync::Mutex;

use codex_utils_path_uri::PathUri;
use futures::executor::block_on;
use pretty_assertions::assert_eq;

use super::*;

#[derive(Debug, Default)]
struct RecordingPlanningFileSystem {
    calls: Mutex<Vec<String>>,
}

#[derive(Clone, Debug)]
struct TestRoot(PathUri);

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestPath(String);

impl PlanningFileSystem for RecordingPlanningFileSystem {
    type Root = TestRoot;
    type ResolvedPath = TestPath;

    async fn open_root(&self, root: &PathUri) -> Result<Self::Root, TransactionFileSystemError> {
        self.calls.lock().unwrap().push(format!("root:{root}"));
        Ok(TestRoot(root.clone()))
    }

    async fn resolve(
        &self,
        root: &Self::Root,
        model_path: &str,
    ) -> Result<Self::ResolvedPath, TransactionFileSystemError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("resolve:{}:{model_path}", root.0));
        if model_path.split('/').any(|component| component == "..") {
            return Err(TransactionFileSystemError::InvalidModelPath {
                path: model_path.to_string(),
                reason: "parent traversal is not allowed".to_string(),
            });
        }
        Ok(TestPath(model_path.to_string()))
    }

    async fn observe(
        &self,
        path: &Self::ResolvedPath,
        limit: ObservationLimit,
    ) -> Result<ObservedPath, TransactionFileSystemError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("observe:{}:{}", path.0, limit.max_bytes));
        Ok(ObservedPath::Present(ObservedFile::new(
            vec![0xef, 0xbb, 0xbf, b'a', b'\r', b'\n'],
            ExecutorFileIdentity {
                namespace: "test".to_string(),
                value: vec![1, 2, 3],
            },
            MetadataSnapshot::new("test-metadata".to_string(), b"mode=0644".to_vec()),
            NonZeroU64::MIN,
            FileKind::File,
        )))
    }

    fn root_identity(
        &self,
        root: &Self::Root,
    ) -> Result<ExecutorRootIdentity, TransactionFileSystemError> {
        Ok(ExecutorRootIdentity {
            namespace: "test-root".to_string(),
            value: root.0.to_string().into_bytes(),
        })
    }

    fn canonical_path_key(
        &self,
        path: &Self::ResolvedPath,
    ) -> Result<CanonicalPathKey, TransactionFileSystemError> {
        Ok(CanonicalPathKey {
            namespace: "test-path".to_string(),
            value: path.0.as_bytes().to_vec(),
        })
    }
}

#[test]
fn planning_boundary_preserves_path_uri_model_string_and_exact_bytes() {
    let file_system = RecordingPlanningFileSystem::default();
    let root_uri = PathUri::parse("file:///workspace").unwrap();

    let (root_identity, path_key, observed) = block_on(async {
        let root = file_system.open_root(&root_uri).await.unwrap();
        let path = file_system.resolve(&root, "src/main.rs").await.unwrap();
        let observed = file_system
            .observe(&path, ObservationLimit { max_bytes: 1024 })
            .await
            .unwrap();
        (
            file_system.root_identity(&root).unwrap(),
            file_system.canonical_path_key(&path).unwrap(),
            observed,
        )
    });

    let expected_file = ObservedFile::new(
        vec![0xef, 0xbb, 0xbf, b'a', b'\r', b'\n'],
        ExecutorFileIdentity {
            namespace: "test".to_string(),
            value: vec![1, 2, 3],
        },
        MetadataSnapshot::new("test-metadata".to_string(), b"mode=0644".to_vec()),
        NonZeroU64::MIN,
        FileKind::File,
    );
    assert_eq!(
        root_identity,
        ExecutorRootIdentity {
            namespace: "test-root".to_string(),
            value: b"file:///workspace".to_vec(),
        }
    );
    assert_eq!(
        path_key,
        CanonicalPathKey {
            namespace: "test-path".to_string(),
            value: b"src/main.rs".to_vec(),
        }
    );
    assert_eq!(observed, ObservedPath::Present(expected_file));
    assert_eq!(
        file_system.calls.into_inner().unwrap(),
        vec![
            "root:file:///workspace".to_string(),
            "resolve:file:///workspace:src/main.rs".to_string(),
            "observe:src/main.rs:1024".to_string(),
        ]
    );
}

#[test]
fn executor_resolution_fails_closed_on_parent_traversal() {
    let file_system = RecordingPlanningFileSystem::default();
    let root_uri = PathUri::parse("file:///workspace").unwrap();

    let error = block_on(async {
        let root = file_system.open_root(&root_uri).await.unwrap();
        file_system.resolve(&root, "src/../secret").await
    })
    .unwrap_err();

    assert_eq!(
        error,
        TransactionFileSystemError::InvalidModelPath {
            path: "src/../secret".to_string(),
            reason: "parent traversal is not allowed".to_string(),
        }
    );
}

#[test]
fn exact_digest_distinguishes_raw_byte_representation() {
    assert_ne!(
        ExactBytesDigest::new(b"line\n"),
        ExactBytesDigest::new(b"line\r\n")
    );
    assert_ne!(
        ExactBytesDigest::new(b"line\n"),
        ExactBytesDigest::new(b"\xef\xbb\xbfline\n")
    );
}
