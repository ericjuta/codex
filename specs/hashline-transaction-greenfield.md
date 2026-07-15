# Design Recoverable Hashline File Transactions

- Branch: main (spec-only; implementation branch TBD)
- Status: Draft
- Owner(s): Codex
- Created: 2026-07-15
- Last Updated: 2026-07-15 07:31Z
- Links: [Current Hashline integration](../codex-rs/docs/hashline_tool_integration_spec.md) | [Prior Hashline hardening](hashline-audit-hardening.md)

This ExecPlan is the source of truth for a greenfield Hashline transaction
design. It specifies the target contract and staged implementation plan; this
spec commit does not change runtime behavior.

## Purpose / Big Picture

Hashline can validate and prepare several file mutations before handing them to
the apply-patch engine, but its current create mode is call-wide and the
underlying filesystem application is not a transaction. A batch cannot express
an existing-file update and a new-file creation with explicit per-file semantics,
and a later application failure can leave an earlier mutation visible.

The greenfield design makes a typed transaction the core abstraction. Each file
declares whether it is created, updated, deleted, or moved, along with an explicit
precondition. The engine plans every after-image before writing, stages durable
state, and either completes the batch or leaves enough journal evidence to roll
back or recover it deterministically.

The contract deliberately distinguishes validation atomicity, crash recovery,
and simultaneous multi-file visibility. Ordinary filesystems do not provide a
portable primitive that makes unrelated paths change at one instant. The first
version therefore promises validation-first, crash-consistent, recoverable
transactions for regular UTF-8 workspace files; it does not claim database-style
isolation from non-cooperating processes.

Success means:

- one typed request can mix create, update, delete, and move operations without a
  global create flag or header inference;
- every path, content, and destination precondition is checked before the first
  visible mutation;
- preview and commit use the same deterministic planned transaction and bounded
  response model;
- injected failures at every commit phase either restore all before-images or
  leave a durable, idempotently recoverable journal;
- stale input, conflicting paths, unsupported filesystems, symlinks, directories,
  and oversized transactions fail with no workspace mutation;
- integration tests cover local and remote/foreign app-exec environments on the
  platforms supported by Codex;
- documentation and tool output state that arbitrary external readers can observe
  per-path commit progress and non-cooperating writers cannot be fully isolated.

Non-goals for the first version:

- instantaneous all-path visibility to arbitrary external readers;
- directory-tree, symlink, hard-link, device-file, or binary-file editing;
- compatibility aliases for the existing Hashline patch grammar inside the new
  transaction engine;
- distributed transactions across multiple selected environments;
- weakening existing sandbox approval, path, or environment-selection checks.

## Progress

- [x] (2026-07-15 07:17Z) Verify the current Hashline create/update and apply-patch atomicity boundaries.
- [x] (2026-07-15 07:19Z) Define the greenfield API, guarantee levels, recovery model, and staged implementation plan.
- [x] (2026-07-15 07:31Z) Resolve independent review findings for file identity, path races, executor capabilities, recovery semantics, and path types.
- [x] (2026-07-15 08:26Z) Establish the separate crate, executor-owned read-only planning boundary, complete transaction capability traits, and exact-byte identity/metadata evidence types.
- [x] (2026-07-15 08:42Z) Add the typed mixed-operation planner core, conflict-free canonicalization, exact-byte before/after evidence, hard planning limits, and deterministic plan digests without exposing mutation capabilities.
- [ ] Implement the typed planner as a separate crate with no filesystem writes.
- [ ] Implement the staged executor, durable journal, rollback, and startup recovery.
- [ ] Add the core tool adapter, remote-environment capability boundary, and bounded responses.
- [ ] Run focused, fault-injection, cross-platform, and integration validation.
- [ ] Record implementation outcomes, review findings, rollout evidence, and residual risks.

## Surprises & Discoveries

- Observation: Hashline's current `create` value is one `PatchArgs` field and is
  applied to every multi-file section.
  Evidence: `codex-rs/core/src/tools/handlers/hashline.rs`, especially
  `PatchArgs` and `handle_multi_file_patch`.
- Observation: Hashline already prepares every current multi-file section before
  emitting one apply-patch envelope, so a stale final section fails before that
  handoff.
  Evidence: `hashline_review_multi_file_stale_final_section_is_atomic` in
  `codex-rs/core/tests/suite/hashline.rs`.
- Observation: one apply-patch envelope is not an all-or-nothing filesystem
  transaction; the standalone apply-patch suite intentionally proves that an
  earlier add remains after a later update fails.
  Evidence: `test_apply_patch_cli_failure_after_partial_success_leaves_changes`
  in `codex-rs/apply-patch/tests/suite/tool.rs`.
- Observation: the workspace already provides `sha2`, `serde`, `tempfile`, and
  `uuid`, but dependency and platform APIs must still be rechecked when each
  implementation milestone begins.
  Evidence: `codex-rs/Cargo.toml` and current crate manifests.
- Observation: the selected environment may be remote and may run a different OS
  from app-server, so a host-only journal or local-path assumption is invalid.
  Evidence: repository remote-test policy and existing environment-aware
  apply-patch routing.
- Observation: exact content alone cannot identify a file after another process
  replaces the path with identical bytes or changes metadata/link topology.
  Evidence: independent design review of the initial spec draft; the transaction
  guard now requires executor identity, metadata, and single-link evidence.
- Observation: the current executor filesystem abstraction does not expose the
  handle-relative traversal, rename, sync, identity, and recovery primitives this
  durability contract needs.
  Evidence: current executor/apply-patch interfaces; the new tool must remain
  unavailable until one complete environment capability proves those semantics.

## Decision Log

- Decision: Put the transaction planner and executor in a new
  `codex-hashline-transaction` crate, leaving only tool orchestration in
  `codex-core`.
  Rationale: transaction planning, journaling, recovery, and fault injection form
  a reusable concept and should not add another subsystem to the already large
  core crate.
  Date/Author: 2026-07-15 / Codex
- Decision: Model create, update, delete, and move as explicit tagged variants.
  Rationale: a global boolean and header-shape inference cannot express mixed
  operations clearly or attach the right precondition to each path.
  Date/Author: 2026-07-15 / Codex
- Decision: Use a full SHA-256 digest of exact source bytes as the content
  precondition and plan component; retain compact line/block anchors only as edit
  locators within a fully guarded file.
  Rationale: the transaction boundary must detect BOM, EOL, and other raw-byte
  changes and must not treat compact non-cryptographic display anchors as its
  concurrency guard. `sha2` is already a workspace dependency.
  Date/Author: 2026-07-15 / Codex
- Decision: Bind each existing source to an executor-derived file identity and
  metadata fingerprint in addition to its exact content digest, and reject hard
  links in the first version.
  Rationale: identical bytes do not prove that a path still names the file object
  that was planned. Recovery must distinguish inode/file-ID replacement, metadata
  changes, and additional links before it overwrites or removes anything.
  Date/Author: 2026-07-15 / Codex
- Decision: Offer explicit `Preview`, `Commit`, and `CommitPreviewed` request
  variants instead of `dry_run` or positional boolean parameters.
  Rationale: the callsite states its intent. `CommitPreviewed` can require the
  digest returned by preview, while `Commit` retains a one-call workflow.
  Date/Author: 2026-07-15 / Codex
- Decision: Define transaction operations as a conflict-free set and preserve
  ordering only inside one file's edit list.
  Rationale: rejecting overlapping source/destination paths avoids hidden
  order-dependent behavior; a move plus content edits remains one typed operation.
  Date/Author: 2026-07-15 / Codex
- Decision: Promise recoverable convergence to all-before, all-after, or an
  evidence-preserving manual-recovery state, not simultaneous multi-file visibility
  or isolation from arbitrary external writers.
  Rationale: portable filesystems provide atomic replacement for individual paths,
  not a global commit primitive. Stronger visibility requires cooperative readers
  resolving a versioned tree through one atomic root pointer.
  Date/Author: 2026-07-15 / Codex
- Decision: Make the selected environment own transaction staging and journal
  storage and every source/destination mutation through one complete transaction
  filesystem capability.
  Rationale: local host storage cannot recover a transaction executed by a remote
  environment. Handle-relative no-follow traversal, identity checks, atomic
  per-path replacement, directory sync, metadata restoration, journaling, and
  recovery are one platform-specific security and durability boundary.
  Date/Author: 2026-07-15 / Codex
- Decision: Accept model-generated paths as `String`, convert protocol paths to
  `PathUri`, and perform fail-closed native-path resolution only inside the
  selected executor.
  Rationale: this follows repository path-type policy and prevents host-native
  `PathBuf` assumptions from corrupting foreign-executor paths. Persistent journals
  must use environment-owned durable path keys, not persisted `PathUri` values.
  Date/Author: 2026-07-15 / Codex

## Context and Orientation

Current Hashline tool schemas and orchestration live in
`codex-rs/core/src/tools/handlers/hashline.rs`. Patch parsing and after-image
construction are split across the `hashline_patch*.rs` sibling modules. Existing
multi-file handling prepares mutations, converts them to one apply-patch text
envelope, and routes that envelope through
`codex-rs/core/src/tools/handlers/apply_patch.rs`.

The apply-patch engine validates its input, but application is a sequence of
ordinary filesystem operations. Its public regression suite confirms that a
failure after an earlier successful operation does not undo the earlier change.
The prior Hashline hardening spec therefore calls its behavior safer multi-file
editing, not an atomic filesystem transaction.

The greenfield engine uses these terms:

- A mutation is one explicit operation on one file identity: create, update,
  delete, or move.
- A plan is the immutable, fully validated set of before-images, after-images,
  metadata actions, and path preconditions derived from a request.
- A plan digest is a deterministic SHA-256 digest over the canonical plan,
  including environment/root identity, normalized paths, exact before/after
  digests, executor file identity, observed metadata/link evidence, operation
  variants, and metadata policy.
- A journal is the durable state machine used to finish or roll back a commit
  after an error or process restart.
- Validation atomicity means no visible write occurs until every mutation has a
  valid plan.
- Recoverable outcome means an interrupted commit can deterministically converge
  to all-before, all-after, or a durable non-destructive `RecoveryRequired` state
  that preserves unknown content and the evidence needed for manual resolution.
- Simultaneous visibility would mean no observer can see an intermediate set of
  paths. This design does not promise it for ordinary workspace files.

Relevant repository rules:

- new functionality should avoid expanding `codex-core` when a focused crate is
  appropriate;
- new Rust modules should stay below the repository size guidance and expose a
  small explicit crate API;
- tests must use `just test`, and agent-logic changes require core integration
  coverage built with auto-environment helpers;
- dependency changes require `just bazel-lock-update`, and compile-time resources
  require matching Bazel data declarations;
- local, macOS, Linux, Windows, and foreign app/exec OS combinations remain in
  scope unless a capability is explicitly unavailable.

## Target Transaction Contract

The public Rust API should use named types and tagged enums. Exact serialization
names may be adjusted during implementation, but the semantics must remain:

    TransactionRequest {
        environment,
        root,
        action,
        mutations,
        limits,
    }

    TransactionAction =
        Preview
        | Commit
        | CommitPreviewed { expected_plan_digest }

    FileMutation =
        Create { path, contents, precondition: Absent }
        | Update { path, expected, edits }
        | Delete { path, expected }
        | Move {
            source,
            expected,
            destination,
            destination_precondition: Absent,
            edits,
        }

    ExpectedFile {
        kind: RegularFile,
        exact_content_sha256,
    }

    ObservedFile {
        executor_identity,
        metadata_fingerprint,
        link_policy: SingleLink,
        exact_content_sha256,
    }

`Create.contents` and all edited files are UTF-8 text in the first version.
`Update.edits` and `Move.edits` are ordered anchored edit operations compiled by
the planner against the exact guarded bytes. A move with edits is one mutation;
callers must not express it as separate move and update entries.

Model-generated `root`, `path`, `source`, and `destination` values deserialize as
ordinary `String`s and may use relative or absolute syntax for the executor's OS.
Before the environment is online, shared code validates only representation-neutral
limits and does not guess its path convention. Core converts selected-environment
roots to `PathUri`; any new exec-server protocol field also uses `PathUri`. The
selected executor resolves tool strings within that root through a handle-bound,
no-follow capability, rejects absolute paths outside it, and returns opaque
`ResolvedTransactionPath` and `ObservedFile` values to the planner.
Host-local implementations may use `AbsolutePathBuf` internally, but neither
`PathBuf` nor executor-native strings cross the app/exec boundary. Security-relevant
conversion failures fail closed, and URI diagnostics render as URIs.

Journals do not persist `PathUri`. `TransactionFileSystem` converts resolved paths
to opaque, executor-native durable path keys that it alone serializes and resolves
during recovery. These keys must remain rooted, no-follow, and unambiguous across
executor restarts without exposing native path assumptions to shared protocol code.
Do not store `PathUri` in rollouts, databases, or other persistent records.

The request must have hard caps for mutation count, total input bytes, total
planned before/after bytes, per-file bytes, edit count, path length, preview
bytes, and response bytes. The exact limits belong in named constants and tests,
not implicit allocator behavior.

The planner rejects:

- empty transactions;
- duplicate or aliased source paths;
- duplicate destinations or a destination that overlaps another source;
- paths outside the selected root, absolute-path ambiguity, symlink traversal,
  non-regular files, hard-linked files, and directories;
- a create or move destination that already exists;
- a missing update/delete/move source;
- any exact-byte digest mismatch or anchored edit mismatch;
- transactions exceeding a hard cap;
- environments that cannot provide the required staging, journal, sync, and
  recovery capabilities, stable file identity, metadata fingerprints, and
  handle-relative no-follow path operations.

The response contains transaction ID, plan digest, operation counts, bounded
per-file before/after digests, bounded previews, commit/recovery state, and
actionable failures. It never returns complete large before-images, after-images,
journals, or raw recovery backups to model context.

## Atomicity and Failure Contract

| Guarantee | Required behavior |
|---|---|
| Validation failure | No visible mutation and no durable commit journal |
| Preview | No visible mutation; return the deterministic bounded plan summary |
| Successful commit | Every requested final path and exact digest is present |
| Ordinary commit error | Roll back applied steps in reverse order before returning when possible |
| Process or host crash | Preserve a journal that recovery can replay idempotently or quarantine as `RecoveryRequired` |
| Recovery failure | Preserve journal and backups; return `RecoveryRequired` with safe operator instructions |
| Concurrent cooperating transaction | Serialize through environment-owned path locks in canonical order |
| Concurrent non-cooperating writer | Detect where possible through final checks; do not claim complete isolation |
| Arbitrary external reader | May observe per-path commit progress; simultaneous visibility is not promised |

The journal state machine is `Preparing -> Prepared -> Committing -> Committed ->
Cleaning -> Complete`, with `RollingBack`, `RolledBack`, and `RecoveryRequired`
failure states. Every transition is persisted and synced before the next
irreversible filesystem step. Recovery uses journal state plus actual before/after
digests, executor file identity, metadata fingerprint, and link policy to infer
whether each step is pending, applied, or externally disturbed. Identical bytes on
a different file identity are an external disturbance, not proof of safety.

## Plan of Work

### Milestone 1: Typed planner with no writes

Scope: introduce the separate crate, request types, validation rules, deterministic
planning, limits, and bounded preview output. This milestone performs no workspace
mutation and is independently reviewable.

Files and interfaces:

- `codex-rs/hashline-transaction/Cargo.toml` and `BUILD.bazel`: new crate metadata.
- `codex-rs/hashline-transaction/src/lib.rs`: minimal exported planning API.
- sibling private modules for request types, path/conflict validation, digesting,
  anchored-edit compilation, limits, and response summaries.
- a `TransactionFileSystem` capability contract whose read-only planning subset
  opens a selected root, resolves every component without following links, and
  returns opaque resolved-path handles plus stable identity/metadata evidence.
- dedicated unit/property test files and fixtures within the new crate.

Work:

1. Define `TransactionRequest`, `TransactionAction`, `FileMutation`,
   `ExpectedFile`, `PlannedTransaction`, and typed errors.
2. Define the complete `TransactionFileSystem` contract before stabilizing the
   crate API. It must own root/path resolution, identity and metadata inspection,
   locks, staging, journals, sync, per-path replacement/removal/move, restoration,
   and recovery; do not bind shared code to host `std::fs` paths.
3. Compile all per-file edits into exact after-images while preserving raw byte
   representation and requiring exact before-byte digests.
4. Record executor identity, metadata fingerprint, and single-link evidence for
   each existing source and destination precondition.
5. Canonicalize the conflict-free plan for a deterministic digest while preserving
   edit order within each file.
6. Enforce hard caps before retaining large buffers and return bounded summaries.

Acceptance:

- deep-equality tests cover mixed operations, conflicts, byte-representation
  changes, stale digests, identical-byte identity replacement, metadata changes,
  hard links, invalid anchors, limits, and deterministic plan digests;
- a planner test proves that no filesystem mutation method is available through
  the planning dependency;
- capability contract tests prove fail-closed `String`/`PathUri` conversion and
  handle-relative no-follow traversal for supported environment implementations;
- `just test -p codex-hashline-transaction` passes.

### Milestone 2: Durable executor and recovery

Scope: add environment-owned locking, staging, journal transitions, commit,
rollback, and restart recovery behind fault-injectable interfaces.

Files and interfaces:

- private executor, journal, staging, locking, and recovery modules in
  `codex-rs/hashline-transaction/src/`;
- platform/environment implementations under the existing filesystem/runtime
  abstraction chosen during implementation;
- fault-injection tests that restart from every durable transition.

Work:

1. Acquire transaction/path locks in canonical order, retain stable root/parent
   handles, and revalidate content, identity, metadata, and link count before
   creating visible mutations.
2. Stage after-images and same-filesystem backups, preserving intended file mode;
   sync staged files, journal records, and affected parent directories where the
   platform exposes meaningful durability primitives.
3. Commit individual paths only through handle-relative no-follow operations while
   recording enough progress to distinguish old, new, and externally disturbed
   identities after restart.
4. On an ordinary error, roll back in reverse order. On restart, idempotently
   choose finish or rollback according to the journal policy and observed digests.
5. Retain journal/backups whenever automatic recovery cannot prove a safe action.

Acceptance:

- fault injection at every transition converges to all-before or all-after state,
  or to an explicit `RecoveryRequired` state with intact evidence;
- repeated recovery is idempotent;
- stale, replaced-with-identical-bytes, metadata-changed, hard-linked, or otherwise
  externally disturbed paths are never silently overwritten during recovery;
- platform-focused crate tests pass on Linux, macOS, and Windows CI.

### Milestone 3: Tool adapter and integration coverage

Scope: expose the engine through a bounded Hashline transaction tool while keeping
the existing Hashline patch surface available during evaluation.

Files and interfaces:

- `codex-rs/core/src/tools/handlers/hashline_transaction.rs`: schema, permission,
  environment selection, bounded output, and engine adapter.
- focused additions to tool registry/spec planning modules.
- new exec-server protocol/runtime capability plumbing where remote execution is
  required; protocol paths use `PathUri`, while model tool paths remain `String`.
- `codex-rs/core/tests/suite/hashline_transaction.rs`: agent integration tests
  using `TestCodexBuilder::build_with_auto_env()` and remote-test guidance.

Work:

1. Define tagged JSON schemas matching the Rust variants; do not expose opaque
   positional booleans.
2. Reuse existing sandbox approval and selected-environment routing for every
   source and destination. Route stage, journal, backup, sync, and recovery work
   through the selected environment's complete `TransactionFileSystem` capability;
   do not emulate missing remote primitives on the app-server host.
3. Return model-visible progress only after durable state transitions and cap every
   per-file and aggregate response collection.
4. Add preview, immediate commit, previewed commit, stale-plan, mixed-operation,
   rollback, recovery, remote-environment, and bounded-output integration tests.
5. Add observability for counts, durations, states, rollback, and recovery without
   logging paths or file contents unless existing policy explicitly permits them.

Acceptance:

- a mixed create/update/delete/move integration test commits every expected final
  digest;
- a stale final mutation proves no visible mutation begins;
- injected mid-commit failure proves rollback or restart recovery;
- `CommitPreviewed` rejects a plan-digest mismatch without writing;
- focused `codex-core` Hashline transaction tests pass in local and auto-env modes.

### Milestone 4: Gated rollout and contract decision

Scope: gather evidence under an opt-in feature, then decide whether the new
transaction tool replaces or complements current Hashline patching.

Work:

1. Introduce an experimental feature disabled by default and keep current Hashline
   behavior unchanged.
2. Run preview/shadow planning against representative multi-file edit workloads and
   measure validation failures, plan sizes, latency, rollback, and recovery.
3. Exercise actual commits in dedicated temporary workspaces, including forced
   process termination and remote executor restarts.
4. Review platform capability gaps and model tool-use quality before changing the
   default tool surface.
5. Record the migration/deprecation decision in this spec and the relevant protocol
   documentation.

Acceptance:

- no recovery journal remains unexpectedly pending after the burn-in window;
- bounded response and transaction-size metrics remain within their caps;
- reviewers approve the documented guarantee language and platform matrix;
- any default-surface change lands separately from the engine implementation.

## Interfaces and Dependencies

Local interfaces:

- `codex_hashline_transaction::plan`:
  - Inputs: typed request with model paths as `String`, selected root as `PathUri`,
    and the read-only planning view of `TransactionFileSystem`.
  - Outputs: immutable `PlannedTransaction` and bounded summary.
  - Failures: typed path, conflict, stale, encoding, anchor, limit, and capability errors.
- `codex_hashline_transaction::execute`:
  - Inputs: planned transaction and the complete environment-owned
    `TransactionFileSystem` capability.
  - Outputs: committed, rolled-back, already-committed, or recovery-required result.
  - Failures: typed staging, sync, commit, rollback, external-change, and recovery errors.
- `TransactionFileSystem`:
  - Resolution: open one selected `PathUri` root, resolve model path strings within
    it component by component without following links, and retain opaque stable
    root/parent handles for planning and commit.
  - Observation: read exact bytes and return executor identity, metadata
    fingerprint, file kind, and link topology from the same guarded object.
  - Coordination: lock the canonical path set and revalidate guarded objects before
    visible mutation.
  - Durability: allocate same-filesystem stage/backup objects, write and sync bytes,
    persist and sync journal transitions, sync parent directories, and report when
    the platform cannot prove a required durability primitive.
  - Mutation: create, atomically replace one path, remove, and move only relative to
    retained handles with no-follow semantics; preserve or restore required metadata.
  - Recovery: serialize executor-native durable path keys and identity evidence,
    reopen them after restart, inspect actual state, finish, roll back, quarantine,
    and clean up idempotently.
  - Failures: unsupported capability, path conversion, identity instability, link
    policy, lock contention, storage exhaustion, sync, permission, external change,
    and platform errors.

Planning and execution run inside the selected environment. A `PlannedTransaction`
or native handle never crosses app-server/exec-server protocol boundaries; only the
typed request, `PathUri` root, bounded summary, and plan digest do. A later
`CommitPreviewed` request replans inside that environment and compares the complete
digest before committing.

Any async capability trait uses native RPITIT methods with explicit `Send` bounds
on returned futures. Do not introduce `#[async_trait]` or
`#[allow(async_fn_in_trait)]`.

Expected workspace dependencies:

- `sha2`: exact-byte and canonical-plan SHA-256 digests; already a workspace dependency.
- `serde`: tagged request, plan, journal, and result records; already a workspace dependency.
- `uuid`: collision-resistant transaction IDs; already a workspace dependency.
- `tempfile`: test and staging helpers where its persistence semantics meet the
  platform contract; already a workspace dependency.

Do not add a lock or filesystem dependency solely from assumption. Inspect its
source, platform behavior, and license before adoption. Any Cargo dependency
change must update `Cargo.lock` and `MODULE.bazel.lock` in the same change.

## Concrete Steps

From the repository root:

    git status --short --branch
    rg -n "handle_multi_file_patch|apply_patch_for_hashline_mutations" codex-rs/core/src/tools/handlers
    rg -n "failure_after_partial_success" codex-rs/apply-patch/tests

After each Rust milestone, from `codex-rs`:

    just test -p codex-hashline-transaction
    just test -p codex-core hashline_transaction --no-capture
    just fix -p codex-hashline-transaction
    just fix -p codex-core
    just fmt

When dependency metadata changes, from the repository root:

    just bazel-lock-update

Do not run direct `cargo test`. Ask before the complete `just test` suite because
the implementation changes core. Do not rerun tests after the final `just fix` or
`just fmt`; inspect their output and the resulting diff instead, per repository
policy.

## Validation and Acceptance

Automated validation:

- planner deep-equality and deterministic-digest tests;
- path alias, traversal, symlink, duplicate destination, stale byte digest, invalid
  anchor, encoding, and every hard-cap rejection test;
- fault-injection tests before and after every journal transition and path commit;
- process-restart tests proving idempotent finish, rollback, and manual-recovery
  preservation;
- integration tests for mixed operations, previewed commit, permission denial,
  identical-byte file replacement, metadata/link changes, path-component races,
  remote environments, foreign app/exec OSes, and bounded model-visible output;
- existing Hashline and apply-patch suites remain green while both surfaces coexist;
- `git diff --check`, final diff inspection, and final worktree status.

Observable acceptance scenarios:

1. Preview a mixed transaction and observe no file changes plus a deterministic
   plan digest and bounded per-file summary.
2. Commit that exact transaction and observe every expected path and SHA-256 digest.
3. Change one source after preview and observe `CommitPreviewed` reject the batch
   without writing.
4. Replace one source with a different file identity but identical bytes and
   observe commit and recovery refuse to overwrite it.
5. Race a path component with a symlink/directory replacement and observe the
   handle-bound operation remain inside the selected root or fail closed.
6. Terminate the executor after each injected commit step, restart recovery, and
   observe all-before, all-after, or explicit `RecoveryRequired` with intact backup
   evidence.
7. Run the same scenarios through a foreign/remote executor and observe identical
   API semantics or an explicit unsupported-capability error before mutation.

## Idempotence and Recovery

Planning is pure and repeatable for an unchanged filesystem snapshot. Transaction
IDs distinguish attempts, while the plan digest identifies exact semantic work.
Replaying a completed transaction ID returns `AlreadyCommitted` only after verifying
the recorded final digests. It never reapplies edits blindly.

Recovery is a durable state machine, not cleanup-by-best-effort. Every recovery
step compares actual content, executor file identity, metadata fingerprint, and
link topology against journaled before/after evidence before acting. Unknown or
identical-byte-but-replaced objects produce `RecoveryRequired`; they are preserved,
never overwritten. Repeated recovery calls are safe and converge on the same state.

Backout during gated rollout is disabling the experimental tool and recovering or
manually resolving every non-complete journal before removing code. Reverting the
feature must not delete unresolved journals or backups.

## Rollout and Operations

- Feature flag: experimental `hashline_transactions`, disabled by default.
- Migration: none initially; current Hashline patch calls keep their behavior.
- Runtime capability: selected environments advertise handle-relative no-follow
  resolution, stable identity, metadata/link inspection, transaction staging,
  atomic per-path mutation, durable journal, directory sync, locking, and recovery
  support before the tool is offered.
- Monitoring: transaction count and duration by terminal state, preflight failure
  class, staged bytes, rollback count, recovery count/age, and pending journal age.
- Privacy: do not log file contents, patch payloads, before-images, or recovery
  backups; path logging follows existing tool-event policy.
- Publication: engine, adapter, rollout, and any default-surface change should be
  separate reviewable commits or PRs.

## Risks and Open Questions

- Risk: journal or backup exhaustion can turn an ordinary error into manual recovery.
  Mitigation: reserve/check required capacity before `Prepared`, cap transaction
  bytes, and retain explicit recovery evidence.
- Risk: platform rename, sync, open-handle, and permission behavior differs.
  Mitigation: environment capability probes, platform-specific implementations,
  and fault-injection CI on Linux, macOS, and Windows.
- Risk: a non-cooperating writer can race after final validation.
  Mitigation: canonical locks for cooperating transactions, immediate per-path
  content/identity/metadata rechecks, rollback on detected disturbance, and
  explicit non-isolation language.
- Risk: a path component can be replaced between path validation and mutation.
  Mitigation: require executor-native root/parent handles and component-wise
  no-follow operations that bind validation and mutation to the guarded directory
  objects. Environments without that primitive do not expose the tool.
- Risk: rollback can fail after external interference.
  Mitigation: never overwrite unknown content; preserve journal/backups and return
  `RecoveryRequired` with affected paths and safe next actions.
- Risk: transaction responses or journals can become unbounded.
  Mitigation: hard request/plan/response caps and compact per-file summaries;
  journals store only data required for deterministic recovery.
- Open question: can every supported executor implement the complete
  `TransactionFileSystem` identity, handle, mutation, sync, and recovery contract?
  Owner/next step: milestone 1 platform feasibility spike; unsupported environments
  must fail capability negotiation before planning or mutation.
- Open question: should a remote executor recover pending journals automatically at
  startup or only before the next transaction in the same root?
  Owner/next step: milestone 2 failure-mode prototype.
- Open question: should successful transaction receipts persist across restarts for
  `AlreadyCommitted`, and for how long?
  Owner/next step: define retention and privacy bounds before journal schema review.
- Open question: if true simultaneous visibility becomes required, can the product
  constrain all readers to a versioned-tree/root-pointer abstraction?
  Owner/next step: separate architecture proposal; it is outside this file API.

## Outcomes & Retrospective

- Outcome: the greenfield contract and guarantee boundaries are specified without
  changing current runtime behavior.
  Evidence: this spec and the inspected current Hashline/apply-patch source and tests.
  Remaining: all implementation, fault injection, cross-platform proof, rollout,
  and final API review.
- Outcome: the design avoids adding the transaction subsystem directly to
  `codex-core` and avoids describing one apply-patch envelope as filesystem atomic.
  Evidence: crate boundary and atomicity decisions above.
  Remaining: validate the proposed environment storage interface against local and
  remote executors during milestone 1.
- Outcome: independent review blockers were incorporated before the spec commit.
  Evidence: exact-byte guards are supplemented by executor identity and metadata;
  path mutations require handle-relative no-follow capabilities; recovery includes
  a non-destructive manual state; model, protocol, native, and persistent path types
  are separated explicitly.
  Remaining: prove each capability on supported platform/environment implementations.

## Artifacts and Notes

Current partial-application evidence:

    test_apply_patch_cli_failure_after_partial_success_leaves_changes
    creates created.txt, then fails updating missing.txt, and asserts created.txt remains.

The stronger visibility architecture, if ever required, is a versioned workspace
tree with one atomically replaced root pointer and cooperative readers. It is not a
property that can be retrofitted onto arbitrary direct reads of unrelated paths.

## Revision Notes

- 2026-07-15: Created the greenfield transaction contract, staged implementation
  plan, validation matrix, recovery model, rollout boundary, and open questions.
- 2026-07-15: Addressed independent design review by adding stable file identity,
  metadata/link guards, handle-bound path safety, a complete environment transaction
  capability, manual-recovery semantics, and repository-compliant path types.
