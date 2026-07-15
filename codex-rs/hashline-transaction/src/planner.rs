use std::collections::BTreeMap;
use std::num::NonZeroU64;

use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;

use crate::CanonicalPathKey;
use crate::ExactBytesDigest;
use crate::FileEdit;
use crate::FileKind;
use crate::FileMutation;
use crate::ObservationLimit;
use crate::ObservedFile;
use crate::ObservedPath;
use crate::PlanSummary;
use crate::PlannedMutation;
use crate::PlannedTransaction;
use crate::PlanningFileSystem;
use crate::TransactionAction;
use crate::TransactionFileSystemError;
use crate::TransactionLimits;
use crate::TransactionRequest;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PlanError {
    #[error(transparent)]
    FileSystem(#[from] TransactionFileSystemError),
    #[error("transaction must contain at least one mutation")]
    Empty,
    #[error("transaction {resource} limit exceeded: observed {observed}, limit {limit}")]
    Limit {
        resource: &'static str,
        observed: u64,
        limit: u64,
    },
    #[error("transaction paths `{first}` and `{second}` resolve to the same object")]
    PathConflict { first: String, second: String },
    #[error("transaction expected `{path}` to be absent")]
    ExpectedAbsent { path: String },
    #[error("transaction expected `{path}` to be an existing file")]
    ExpectedExistingFile { path: String },
    #[error("transaction input for `{path}` is stale")]
    Stale { path: String },
    #[error("transaction path `{path}` has unsupported kind {kind:?}")]
    UnsupportedKind { path: String, kind: FileKind },
    #[error("transaction path `{path}` has {link_count} hard links; exactly one is required")]
    HardLink { path: String, link_count: u64 },
    #[error("transaction edits for `{path}` require one replacement, or none for a move")]
    InvalidEdits { path: String },
    #[error("previewed transaction digest does not match the current plan")]
    PlanDigestMismatch {
        expected: ExactBytesDigest,
        actual: ExactBytesDigest,
    },
}

struct ResolvedPath<P> {
    model_path: String,
    handle: P,
    key: CanonicalPathKey,
}

struct PlanState<'a, F: PlanningFileSystem> {
    file_system: &'a F,
    root: &'a F::Root,
    limits: TransactionLimits,
    used_paths: BTreeMap<CanonicalPathKey, String>,
    summary: PlanSummary,
}

impl<'a, F: PlanningFileSystem> PlanState<'a, F> {
    async fn resolve_unique(
        &mut self,
        model_path: String,
    ) -> Result<ResolvedPath<F::ResolvedPath>, PlanError> {
        check_limit(
            "model path bytes",
            model_path.len() as u64,
            self.limits.max_model_path_bytes,
        )?;
        let handle = self.file_system.resolve(self.root, &model_path).await?;
        let key = self.file_system.canonical_path_key(&handle)?;
        check_limit(
            "executor key bytes",
            key.namespace.len().saturating_add(key.value.len()) as u64,
            self.limits.max_executor_key_bytes,
        )?;
        if let Some(first) = self.used_paths.insert(key.clone(), model_path.clone()) {
            return Err(PlanError::PathConflict {
                first,
                second: model_path,
            });
        }
        Ok(ResolvedPath {
            model_path,
            handle,
            key,
        })
    }

    async fn observe(
        &mut self,
        path: &ResolvedPath<F::ResolvedPath>,
    ) -> Result<ObservedPath, PlanError> {
        let observed = self
            .file_system
            .observe(
                &path.handle,
                ObservationLimit {
                    max_bytes: self.limits.max_file_bytes,
                },
            )
            .await?;
        if let ObservedPath::Present(file) = &observed {
            check_limit(
                "file bytes",
                file.contents.len() as u64,
                self.limits.max_file_bytes,
            )?;
            self.charge_before(file.contents.len())?;
        }
        Ok(observed)
    }

    fn require_file(
        &self,
        path: &str,
        expected: ExactBytesDigest,
        observed: ObservedPath,
    ) -> Result<ObservedFile, PlanError> {
        let ObservedPath::Present(file) = observed else {
            return Err(PlanError::ExpectedExistingFile {
                path: path.to_string(),
            });
        };
        if file.kind != FileKind::File {
            return Err(PlanError::UnsupportedKind {
                path: path.to_string(),
                kind: file.kind,
            });
        }
        if file.link_count != NonZeroU64::MIN {
            return Err(PlanError::HardLink {
                path: path.to_string(),
                link_count: file.link_count.get(),
            });
        }
        if file.exact_digest != expected {
            return Err(PlanError::Stale {
                path: path.to_string(),
            });
        }
        Ok(file)
    }

    fn charge_before(&mut self, bytes: usize) -> Result<(), PlanError> {
        self.summary.before_bytes = self.summary.before_bytes.saturating_add(bytes as u64);
        self.check_total()
    }

    fn charge_after(&mut self, bytes: usize) -> Result<(), PlanError> {
        check_limit("file bytes", bytes as u64, self.limits.max_file_bytes)?;
        self.summary.after_bytes = self.summary.after_bytes.saturating_add(bytes as u64);
        self.check_total()
    }

    fn check_total(&self) -> Result<(), PlanError> {
        let observed = self
            .summary
            .before_bytes
            .saturating_add(self.summary.after_bytes);
        if observed > self.limits.max_total_bytes {
            return Err(PlanError::Limit {
                resource: "total bytes",
                observed,
                limit: self.limits.max_total_bytes,
            });
        }
        Ok(())
    }

    async fn plan_mutation(
        &mut self,
        mutation: FileMutation,
    ) -> Result<PlannedMutation<F::ResolvedPath>, PlanError> {
        match mutation {
            FileMutation::Create { path, contents } => {
                let path = self.resolve_unique(path).await?;
                if self.observe(&path).await? != ObservedPath::Absent {
                    return Err(PlanError::ExpectedAbsent {
                        path: path.model_path,
                    });
                }
                self.charge_after(contents.len())?;
                self.summary.creates += 1;
                let after_digest = ExactBytesDigest::new(&contents);
                Ok(PlannedMutation::Create {
                    path: path.handle,
                    path_key: path.key,
                    contents,
                    after_digest,
                })
            }
            FileMutation::Update {
                path,
                expected,
                edits,
            } => {
                let path = self.resolve_unique(path).await?;
                let observed = self.observe(&path).await?;
                let before =
                    self.require_file(&path.model_path, expected.exact_digest, observed)?;
                let contents = compile_edits(&path.model_path, &before.contents, edits, false)?;
                self.charge_after(contents.len())?;
                self.summary.updates += 1;
                let after_digest = ExactBytesDigest::new(&contents);
                Ok(PlannedMutation::Update {
                    path: path.handle,
                    path_key: path.key,
                    before,
                    contents,
                    after_digest,
                })
            }
            FileMutation::Delete { path, expected } => {
                let path = self.resolve_unique(path).await?;
                let observed = self.observe(&path).await?;
                let before =
                    self.require_file(&path.model_path, expected.exact_digest, observed)?;
                self.summary.deletes += 1;
                Ok(PlannedMutation::Delete {
                    path: path.handle,
                    path_key: path.key,
                    before,
                })
            }
            FileMutation::Move {
                source,
                expected,
                destination,
                edits,
            } => {
                let source = self.resolve_unique(source).await?;
                let destination = self.resolve_unique(destination).await?;
                let observed = self.observe(&source).await?;
                let before =
                    self.require_file(&source.model_path, expected.exact_digest, observed)?;
                if self.observe(&destination).await? != ObservedPath::Absent {
                    return Err(PlanError::ExpectedAbsent {
                        path: destination.model_path,
                    });
                }
                let contents = compile_edits(&source.model_path, &before.contents, edits, true)?;
                self.charge_after(contents.len())?;
                self.summary.moves += 1;
                let after_digest = ExactBytesDigest::new(&contents);
                Ok(PlannedMutation::Move {
                    source: source.handle,
                    source_key: source.key,
                    before,
                    destination: destination.handle,
                    destination_key: destination.key,
                    contents,
                    after_digest,
                })
            }
        }
    }
}

pub async fn plan<F: PlanningFileSystem>(
    file_system: &F,
    request: TransactionRequest,
) -> Result<PlannedTransaction<F::Root, F::ResolvedPath>, PlanError> {
    if request.mutations.is_empty() {
        return Err(PlanError::Empty);
    }
    check_limit(
        "mutation count",
        request.mutations.len() as u64,
        request.limits.max_mutations,
    )?;

    let root = file_system.open_root(&request.root).await?;
    let root_identity = file_system.root_identity(&root)?;
    check_limit(
        "executor key bytes",
        root_identity
            .namespace
            .len()
            .saturating_add(root_identity.value.len()) as u64,
        request.limits.max_executor_key_bytes,
    )?;

    let mut state = PlanState {
        file_system,
        root: &root,
        limits: request.limits,
        used_paths: BTreeMap::new(),
        summary: PlanSummary::default(),
    };
    let mut mutations = Vec::with_capacity(request.mutations.len());
    for mutation in request.mutations {
        mutations.push(state.plan_mutation(mutation).await?);
    }
    let summary = state.summary;
    drop(state);
    let plan_digest = digest_plan(
        &request.environment_id,
        &request.root.to_string(),
        &root_identity,
        &mutations,
    );
    if let TransactionAction::CommitPreviewed {
        expected_plan_digest,
    } = &request.action
        && *expected_plan_digest != plan_digest
    {
        return Err(PlanError::PlanDigestMismatch {
            expected: *expected_plan_digest,
            actual: plan_digest,
        });
    }

    Ok(PlannedTransaction {
        environment_id: request.environment_id,
        root_uri: request.root,
        root,
        root_identity,
        action: request.action,
        mutations,
        plan_digest,
        summary,
    })
}

fn compile_edits(
    path: &str,
    before: &[u8],
    edits: Vec<FileEdit>,
    allow_empty: bool,
) -> Result<Vec<u8>, PlanError> {
    let mut edits = edits.into_iter();
    match (edits.next(), edits.next()) {
        (None, None) if allow_empty => Ok(before.to_vec()),
        (Some(FileEdit::ReplaceAll { contents }), None) => Ok(contents),
        _ => Err(PlanError::InvalidEdits {
            path: path.to_string(),
        }),
    }
}

fn check_limit(resource: &'static str, observed: u64, limit: u64) -> Result<(), PlanError> {
    if observed > limit {
        Err(PlanError::Limit {
            resource,
            observed,
            limit,
        })
    } else {
        Ok(())
    }
}

fn digest_plan<P>(
    environment_id: &str,
    root_uri: &str,
    root_identity: &crate::ExecutorRootIdentity,
    mutations: &[PlannedMutation<P>],
) -> ExactBytesDigest {
    let mut digest = PlanDigest::default();
    digest.bytes(b"hashline-transaction-plan-v1");
    digest.bytes(environment_id.as_bytes());
    digest.bytes(root_uri.as_bytes());
    digest.bytes(root_identity.namespace.as_bytes());
    digest.bytes(&root_identity.value);

    let mut ordered = mutations.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| mutation_key(left).cmp(mutation_key(right)));
    digest.number(ordered.len() as u64);
    for mutation in ordered {
        digest.mutation(mutation);
    }
    ExactBytesDigest::from_array(digest.finish())
}

fn mutation_key<P>(mutation: &PlannedMutation<P>) -> &CanonicalPathKey {
    match mutation {
        PlannedMutation::Create { path_key, .. }
        | PlannedMutation::Update { path_key, .. }
        | PlannedMutation::Delete { path_key, .. } => path_key,
        PlannedMutation::Move { source_key, .. } => source_key,
    }
}

#[derive(Default)]
struct PlanDigest(Sha256);

impl PlanDigest {
    fn bytes(&mut self, value: &[u8]) {
        self.number(value.len() as u64);
        self.0.update(value);
    }

    fn number(&mut self, value: u64) {
        self.0.update(value.to_be_bytes());
    }

    fn path_key(&mut self, key: &CanonicalPathKey) {
        self.bytes(key.namespace.as_bytes());
        self.bytes(&key.value);
    }

    fn observed_file(&mut self, file: &ObservedFile) {
        self.bytes(file.exact_digest.as_bytes());
        self.bytes(file.identity.namespace.as_bytes());
        self.bytes(&file.identity.value);
        self.bytes(file.metadata.namespace.as_bytes());
        self.bytes(file.metadata.fingerprint.as_bytes());
        self.number(file.link_count.get());
        self.number(match file.kind {
            FileKind::File => 1,
            FileKind::Directory => 2,
            FileKind::SymbolicLink => 3,
            FileKind::Other => 4,
        });
    }

    fn mutation<P>(&mut self, mutation: &PlannedMutation<P>) {
        match mutation {
            PlannedMutation::Create {
                path_key,
                after_digest,
                ..
            } => {
                self.number(1);
                self.path_key(path_key);
                self.bytes(after_digest.as_bytes());
            }
            PlannedMutation::Update {
                path_key,
                before,
                after_digest,
                ..
            } => {
                self.number(2);
                self.path_key(path_key);
                self.observed_file(before);
                self.bytes(after_digest.as_bytes());
            }
            PlannedMutation::Delete {
                path_key, before, ..
            } => {
                self.number(3);
                self.path_key(path_key);
                self.observed_file(before);
            }
            PlannedMutation::Move {
                source_key,
                before,
                destination_key,
                after_digest,
                ..
            } => {
                self.number(4);
                self.path_key(source_key);
                self.observed_file(before);
                self.path_key(destination_key);
                self.bytes(after_digest.as_bytes());
            }
        }
    }

    fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}
