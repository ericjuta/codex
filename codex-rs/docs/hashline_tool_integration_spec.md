# Hashline Tool Integration Spec

## Purpose

Evaluate `/tmp/hashline` as the reference implementation for hash-anchored file
reading and patching, then define how Codex should integrate it into the
existing toolset without regressing sandboxing, approvals, remote filesystem
support, code-mode tool planning, or the host's approval and sandbox guarantees.

Live proof used for this spec:

| Surface | Live state | Implication |
| --- | --- | --- |
| Reference checkout | `/tmp/hashline`, `main...origin/main`, clean at inspection time | Treat it as reference code, not vendored runtime truth. |
| Codex checkout | `/home/ericjuta/.openclaw/workspace/repos/codex`, branch `feat/hashline-audit-hardening` | Spec is written against the active hardening branch. |
| Existing Codex tool path | `codex-rs/tools` plus `codex-rs/core/src/tools` | Add model-visible definitions in the normal tool planning path. |
| Existing patch path | `codex-rs/apply-patch` plus `core/src/tools/handlers/apply_patch.rs` and `core/src/tools/runtimes/apply_patch.rs` | Any replacement must preserve approval, sandbox, remote FS, telemetry, and event behavior. |
| MCP path | `codex-rs/codex-mcp/src/connection_manager.rs` and `core/src/tools/handlers/mcp.rs` | Hashline can be tried as MCP, but MCP is not the best default integration boundary. |

Related spec: [Hashline Enablement Flags Spec](hashline_enablement_flags_spec.md)
defines the `[features].hashline = true` and `[features].hashline_only = true`
config surface that gates this integration.

## Recommendation

Implement Hashline as an additive native Codex tool namespace first. Do not
replace `apply_patch` in the first stage.

The preferred first landing stage is:

1. Keep parser, hashing, formatting, and text-application logic outside the
   root handler. Handler-private modules are acceptable for the initial hardening
   pass; extract `codex-hashline` when the logic needs a reusable crate boundary.
2. Add native Codex handlers for `hashline.read`, `hashline.write`,
   `hashline.patch`, `hashline.find_block`, `hashline.remove_file`, and
   `hashline.rename_file`.
3. Keep `apply_patch` registered and unchanged.
4. Gate the new tools behind a feature flag, model capability check, or config
   knob until integration tests prove behavior across local, remote, sandboxed,
   and multi-environment sessions.
5. Only consider making Hashline the default patch/edit surface after it has
   parity with the host guarantees currently provided by `apply_patch`.

This path captures Hashline's real advantage: file snapshot hashes plus
line-addressed edit anchors. It avoids routing filesystem mutation through an
external process or MCP server that cannot naturally participate in Codex's
approval and environment model.

## Current State

### Hashline Reference

Hashline provides:

| Capability | Reference files | Notes |
| --- | --- | --- |
| Snapshot read format | `/tmp/hashline/crates/core/src/commands/read.rs`, `/tmp/hashline/crates/core/src/document.rs`, `/tmp/hashline/crates/core/src/hash.rs` | Reference emits `[path#HASH]` plus `line:hash|content`; the native Codex contract emits `[path]#HASH` plus fixed-width 4-hex line anchors and 8-hex file guards. |
| Patch grammar | `/tmp/hashline/crates/core/src/tokenizer.rs`, `/tmp/hashline/crates/core/src/parser.rs` | Supports the Hashline operations and sectioned file operations; the native Codex contract requires fixed-width 4-hex line anchors and 8-hex file/block guards. |
| Patch application | `/tmp/hashline/crates/core/src/commands/patch.rs` | Applies parsed edits against current lines and optional per-line expected hashes. |
| Block finding | `/tmp/hashline/crates/core/src/commands/find_block.rs`, `/tmp/hashline/crates/core/src/block.rs` | Uses extension-driven brace, Markdown section, Python/Verse indentation, and Ruby `end`-pair block rules. |
| CLI/MCP | `/tmp/hashline/crates/core/src/cli.rs`, `/tmp/hashline/crates/core/src/mcp.rs` | Exposes `read`, `patch`, `write`, `find_block`, `remove_file`, and `rename_file`. |
| Reference limitations | `/tmp/hashline/crates/core/src/input.rs`, `/tmp/hashline/crates/core/src/patcher.rs` | Some modules are stubs; CLI command code uses `std::fs`/`memmap2`; file-section header hashes are parsed/merged but not fully enforced by `patch` as the host-level stale-read contract. |

### Codex Tooling

Codex already has the pieces needed for a native integration:

| Area | Current state | Keep or reuse |
| --- | --- | --- |
| Tool model definitions | `codex-rs/tools/src/tool_spec.rs`, `tool_definition.rs`, `tool_executor.rs` | Reuse `ToolSpec`, `ToolExecutor`, `ToolExposure`, namespace merging, and code-mode adaptation. |
| Tool planning | `codex-rs/core/src/tools/spec_plan.rs` | Add Hashline handlers in `add_core_utility_tools` or a focused `add_file_edit_tools` helper. |
| Runtime dispatch | `codex-rs/core/src/tools/registry.rs`, `router.rs`, `handlers/*` | Implement normal `CoreToolRuntime` handlers. |
| Existing patch behavior | `codex-rs/apply-patch`, `core/src/tools/runtimes/apply_patch.rs` | Preserve the approval/sandbox/event contract and mirror it for Hashline mutation. |
| Filesystem abstraction | `codex-rs/file-system`, `codex-rs/exec-server` | Use `ExecutorFileSystem` and `FileSystemSandboxContext`; do not call `std::fs` from native tool runtimes. |
| MCP integration | `codex-rs/codex-mcp/src/connection_manager.rs`, `core/src/tools/handlers/mcp.rs` | Useful for experimentation, not the recommended production boundary. |

## Integration Options

| Priority | Option | Current state | Proposed change | Rationale |
| --- | --- | --- | --- | --- |
| P0 | Native additive namespace | Native Hashline tools are available behind `[features].hashline = true` | Continue hardening parser parity, refreshed patch output, and integration coverage | Best fit for Codex approvals, sandboxing, remote filesystems, code-mode, tests, and telemetry. |
| P1 | External MCP experiment | Hashline has a stdio MCP server | Allow users to configure `/tmp/hashline` or installed `hashline mcp` manually | Fastest smoke path, but weak for default UX because process lifecycle, filesystem permissions, and remote environments sit outside Codex's native edit path. |
| P2 | Full `apply_patch` replacement | `apply_patch` is deeply integrated and compatibility-sensitive | Defer until Hashline has native parity and integration tests | Avoid breaking models, hooks, existing patch approvals, shell interception, and standalone `apply_patch` command behavior. |

## Native Tool Design

### Tool Namespace

Expose a namespace named `hashline` with these first-stage tools:

| Tool | Purpose | Model-visible output |
| --- | --- | --- |
| `hashline.read` | Read a bounded file range with Hashline anchors | `[path]#HASH` plus LF-normalized `line:4-hex|content`, reference-style `hash`/`lines` metadata, and explicit truncation metadata when capped. |
| `hashline.write` | Write normalized content, including empty content, to a new file or overwrite with `force=true` | Success/failure status plus a bounded changed-region `[path]#HASH` excerpt after writing. |
| `hashline.patch` | Apply a Hashline patch to one file, to multiple existing files with `[path]#HASH` sections, or to multiple missing files with `create=true` and `[path]` sections | Success/failure status; existing-file patches require a file guard, create patches do not; dry runs include old/new hashes, file operations, and compact changed-line previews; successful updates include bounded structured rows for the changed region. |
| `hashline.find_block` | Resolve a block around an anchored line | Block span, language guess, `[path]#HASH` file evidence, a `line:4-hex@8-hex` block anchor, reference-style `file`/`line_count`/`block_lines` metadata, and a small LF-normalized anchored excerpt. |
| `hashline.remove_file` | Delete one text file after required file-hash validation | Hashline success/failure status with old file hash after `apply_patch` verifies and applies the delete. |
| `hashline.rename_file` | Move one text file after required file-hash validation | Hashline success/failure status with old/new paths, reference-style `src`/`dst` aliases, and refreshed destination header after `apply_patch` verifies and applies the move. |

Use structured function tools for stage 1. A freeform `hashline_patch` tool can
be added later if model behavior proves better with grammar-constrained patch
bodies.

Successful dry runs for write, patch, remove, and rename tools include
`"success": true` with `"dry_run": true`. Multi-file patch dry runs also mark
each returned per-file entry with `"success": true`. Multi-file patch outputs
include `total_files` and `files_truncated` because the detail array is bounded.

Read and block excerpts have a 24 KiB serialized-content budget in addition to
their line caps. Patch previews and post-patch excerpts have a 4 KiB budget,
and multi-file detail arrays have a 24 KiB budget. If one source line exceeds
an excerpt budget, Hashline returns a UTF-8-safe prefix with a truncation marker
and sets `content_truncated=true` on its structured row.

Empty-file reads and refreshed empty write/create outputs return no line range:
`start_line` and `end_line` are `null`, with empty `content` and `lines`.

### Arguments

The canonical Codex argument names are `path` and `new_path`.
Handlers also accept the reference MCP aliases: `file` for single-path tools and
`src`/`dst` for rename; those aliases are not part of the greenfield contract.

`hashline.read`:

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `path` | string | yes | Resolved relative to selected environment cwd. |
| `start_line` | integer | no | 1-indexed inclusive. |
| `end_line` | integer | no | 1-indexed inclusive. |
| `max_lines` | integer | no | Default 200, hard cap 1000; the serialized byte budget may cap output earlier. |
| `environment_id` | string | only when multiple environments exist | Match existing multi-environment patterns. |

`hashline.write`:

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `path` | string | yes | Resolved relative to selected environment cwd. |
| `content` | string | yes | Complete file content to write. Content is normalized to LF line endings and a leading UTF-8 BOM is stripped. |
| `force` | boolean | no | Defaults to false. Existing files are rejected unless this is true. |
| `dry_run` | boolean | no | Defaults to false. Validates without writing and returns old/new hashes plus a compact changed-line preview when content would change. |
| `environment_id` | string | only when multiple environments exist | Match `apply_patch` environment selection behavior. |

`hashline.patch`:

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `path` | string | yes | Default path/context for the patch request; existing-file edits still require a `[path]#HASH` section. |
| `patch` | string | yes | Hashline ops such as `SWAP 12:1a2b:`, `SWAP 12:1a2b..14:2b3c:`, a required `[path]#HASH` section for existing-file edits, or `[path]` sections with `create=true` for missing-file creation. Payload rows may use README-style `+` prefixes, bare replacement lines, or pasted read-output rows like `1:1a2b\|content`, `>>> 1:1a2b\|content`, or `* 1:1a2b\|content`; `++` emits a literal `+`, `+-` emits a literal `-`, and `+1:1a2b\|content` emits the read-output prefix literally. Sectioned patches also accept `REM` and `MV <path>` file ops; `MV` may be combined with line ops to rename and edit one file section, while `REM` must stand alone. `*** Abort` suppresses an embedded patch without writing. |
| `dry_run` | boolean | no | Defaults to false. Validates without writing and returns old/new hashes plus a byte-bounded changed-line preview. Multi-file details are also byte-bounded. |
| `create` | boolean | no | Defaults to false. When true, every target must be missing and the patch is applied to empty file contents before routing through `apply_patch` add-file handling. Empty patches create zero-byte files. Use `[path]` sections for multi-file creation. |
| `environment_id` | string | only when multiple environments exist | Match `apply_patch` environment selection behavior. |

`hashline.remove_file`:

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `path` | string | yes | File to delete, resolved relative to selected environment cwd. |
| `expected_hash` | string | yes | Required 8-hex logical-text file hash from a Hashline read header. |
| `dry_run` | boolean | no | Validate without deleting. |
| `environment_id` | string | only when multiple environments exist | Match other file tools. |

`hashline.rename_file`:

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `path` | string | yes | Source file, resolved relative to selected environment cwd. |
| `new_path` | string | yes | Destination path, which must be missing. |
| `expected_hash` | string | yes | Required 8-hex logical-text file hash from a Hashline read header. |
| `dry_run` | boolean | no | Validate without renaming. |
| `environment_id` | string | only when multiple environments exist | Match other file tools. |

Rename outputs retain the canonical Codex `path`/`new_path` fields.
Reference-style `src`/`dst` fields may be present but are not part of the greenfield contract.

`hashline.find_block`:

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `path` | string | yes | Same path resolution as `read`. |
| `anchor` | string | yes | Use `line:4-hex`, `block N:`, a unique 4-hex line hash, or a `line:4-hex@8-hex` block anchor returned by `find_block`; bare line numbers are not accepted. |
| `environment_id` | string | only when multiple environments exist | Match other file tools. |

### Output Bounds

The tool must never emit unbounded file content into model context.

Required caps:

| Output | Default cap | Hard cap | Behavior on overflow |
| --- | --- | --- | --- |
| `read` lines | 200 lines | 1000 lines and 24 KiB serialized excerpt | Return selected range plus `truncated=true`, `next_start_line`, and file total when available. |
| `write` changed-region excerpt | 40 lines | 40 lines and 4 KiB serialized excerpt | Return the changed region with refreshed anchors rather than an unconditional first-200-line reread. |
| `patch` preview/excerpt | 40 lines | 40 lines and 4 KiB per preview/excerpt | Return a compact preview, then refreshed anchors for the changed region only. |
| Multi-file patch details | all files | 24 KiB serialized detail array | Return `total_files` and `files_truncated` when not every detail fits. |
| `find_block` excerpt | 80 lines | 300 lines and 24 KiB serialized excerpt | Return span metadata even when excerpt is capped. |
| Any single textual payload | provider/model truncation policy plus local byte cap | less than 10k tokens equivalent | Truncate before building `FunctionCallOutputPayload`. |

This is stricter than the reference Hashline CLI, which reads whole files by
default.

## Crate Boundary

Prefer a new crate over adding reusable Hashline logic to `codex-core`.
Initial handler-private modules under `core/src/tools/handlers/hashline_*.rs`
are acceptable while the native tool surface is still being hardened, as long as
they stay private to the handler and do not become a general core API.

Proposed crate:

| Item | Value |
| --- | --- |
| Directory | `codex-rs/hashline` |
| Crate name | `codex-hashline` |
| Depends on | `serde`, `thiserror`, a reviewed hash dependency such as `xxhash-rust`, optional `regex` only if the parser still needs it |
| Must not depend on | `codex-core`, `codex-exec-server`, `tokio` unless async is proven necessary |

Crate responsibilities:

1. Hash normalized text and lines.
2. Format bounded anchored reads.
3. Parse Hashline patch syntax.
4. Apply parsed edits to an in-memory string.
5. Return a structured `HashlineDelta` that Codex can convert into patch
   approval summaries, telemetry, and model output.

Do not copy the reference CLI filesystem layer directly. The native crate
should accept strings and return strings/deltas. Codex runtime handlers should
own all filesystem reads and writes through `ExecutorFileSystem`.

If the implementation adds `xxhash-rust` or any other Rust dependency, update
`Cargo.lock` and `MODULE.bazel.lock` with the normal `just bazel-lock-update`
flow in the same change.

## Runtime Implementation

### Read Runtime

1. Resolve `environment_id` to a `TurnEnvironment`.
2. Resolve `path` against that environment's cwd.
3. Read bytes through `turn_environment.environment.get_filesystem()`.
4. Reject binary and invalid UTF-8 with a model-facing error.
5. Normalize line endings for hashing while preserving each source line's exact terminator for mutation.
   New lines inherit a local/default terminator; previews remain LF-normalized.
6. Format bounded anchored lines.

### Patch Runtime

Implement a `HashlinePatchRuntime` modeled on `ApplyPatchRuntime`.

Required behavior:

1. Resolve environment and path.
2. Read current file through `ExecutorFileSystem`.
3. Parse patch text using `codex-hashline`.
4. Validate expected anchors:
   - If the patch contains `[path]#HASH`, compare it to the current normalized logical-text
     file hash and fail closed on mismatch.
   - If operations include `line:4-hex`, validate the per-line hash before
     applying the operation.
   - Reject bare line numbers and malformed line anchors; there is no warning fallback.
     Hash-qualified anchors are required for line-addressed mutation.
5. Produce a before/after preview and file-change summary before writing.
6. Preserve each untouched logical line's original CRLF, LF, or CR terminator through
   insertions, deletions, and replacements, including BOM and final-newline variants.
7. Reuse Codex approval and sandbox policy:
   - build approval keys by environment and path;
   - use `FileSystemSandboxContext` derived from the selected attempt;
   - expose hook payloads under a stable hook name such as `hashline.patch`;
   - preserve Guardian review compatibility for file mutations.
8. On approval, write via `ExecutorFileSystem::write_file`.
9. Return refreshed file hash and bounded changed-region anchors.

### Block Runtime

Stage 1 can use the reference heuristic block resolver. If block operations
become important for correctness, prefer a later tree-sitter backed resolver
behind the same `codex-hashline` trait boundary. Keep the trait shape native
and `Send + Sync`.

## Model and Prompting Behavior

The tool guidance should be short and operational:

1. Use `hashline.read` before `hashline.patch` when editing a file not already
   read with Hashline anchors.
2. Use fixed-width `line:4-hex` anchors in patch operations.
3. Re-read when a stale file or line hash is reported.
4. Use `hashline.write` for direct single-file creation or force overwrites,
   `hashline.patch create=true` when creating via line operations, including
   sectioned `[path]` multi-file creation,
   `hashline.remove_file` for single-file deletion, and
   `hashline.rename_file` for single-file moves. Use sectioned `hashline.patch` for
   existing-file multi-file edits, including `REM` and `MV <path>` file ops.

Do not present Hashline as a universal replacement during the additive stage.

## MCP Path

Hashline's MCP server can be used for manual experimentation:

    [mcp_servers.hashline]
    command = "hashline"
    args = ["mcp"]

This should not be the default integration path because:

1. MCP tool calls are routed through `McpHandler`, so filesystem mutation is
   owned by the external server process rather than Codex's native
   `ExecutorFileSystem`.
2. The server's local `std::fs` behavior does not map cleanly to remote
   environments.
3. Codex's patch approval flow has richer file-change summaries than the MCP
   response can guarantee.
4. Startup/config errors become user environment problems instead of build-time
   tested Codex behavior.

Use MCP only to compare model ergonomics and failure messages before native
implementation lands.

## Replacement Criteria

Hashline may replace `apply_patch` as the preferred model-visible edit tool only
after all of these are true:

| Gate | Requirement |
| --- | --- |
| Sandbox parity | Hashline writes obey the same local and remote filesystem sandbox behavior as `apply_patch`. |
| Approval parity | Granular approvals, Guardian reviews, hooks, and cached approvals work for Hashline patches. |
| File operation parity | Add, delete, rename/move, overwrite, zero-byte creation, and multi-file operations are supported or intentionally delegated. Existing-file multi-file `hashline.patch` is supported, including sectioned `REM` and `MV`; multi-file creation is supported with `create=true` and `[path]` sections. |
| Output parity | TUI transcript, app-server events, telemetry, and model-facing outputs are stable and bounded. |
| Compatibility | Existing `apply_patch` shell interception and standalone invocation behavior remain available. |
| Tests | Integration tests cover stale hash failure, successful patch, dry run, multi-environment selection, remote exec-server read/write/patch filesystem routing, sandbox denial, and approval. |

Even after these gates, prefer a staged default switch:

1. Add Hashline tools hidden or feature-gated.
2. Make Hashline visible alongside `apply_patch`.
3. Update model instructions to prefer Hashline for line-anchored edits.
4. Keep `apply_patch` as fallback for at least one release train.
5. Only then consider hiding `apply_patch` from models that consistently use
   Hashline well.

## Test Plan

### `codex-hashline`

Add focused unit tests in sibling `*_tests.rs` files:

| Area | Tests |
| --- | --- |
| Hashing | LF/CRLF/CR normalization, trailing whitespace behavior, empty files, fixed-width line/file hash formatting, and logical-text guard semantics. |
| Read formatting | bounded ranges, truncation metadata, file hash formatting. |
| Parser | all supported ops, malformed ops, fixed-width anchors, conflicting section hashes, formatter-to-pasted-payload round trips, and contamination from `apply_patch` syntax. |
| Apply | swaps, deletes, inserts, mixed-EOL cardinality changes, final-newline/BOM variants, multi-op shifting, stale line/file hashes, block-anchor round trips, and block ops. |

### `codex-core` integration

Prefer integration tests under `core/suite`:

| Scenario | Expected proof |
| --- | --- |
| `hashline.read` on text file | Model receives bounded `[path]#HASH` output with fixed-width anchors. |
| `hashline.write` create/overwrite | File changes through the native tool and response includes a bounded changed-region excerpt with refreshed anchors. |
| `hashline.patch` after read | File changes and response includes a bounded changed-region excerpt with a refreshed hash. |
| Sectioned multi-file `hashline.patch` | Multiple existing files change through one Hashline tool call and response includes per-file refreshed hashes or file-op status. |
| Sectioned multi-file create | Multiple missing files are created through one Hashline tool call and response includes per-file refreshed hashes. |
| Stale file hash | File operations are rejected without writing. |
| Stale line hash | Patch is rejected without writing and points to re-read. |
| Dry run | Changed-line preview is returned and file is unchanged. |
| Multi-environment | `environment_id` selects the correct filesystem. |
| Sandbox denied | Runtime requests/uses approval consistently with `apply_patch`. |
| Remote filesystem | Reads, writes, and patches go through exec-server FS, not host `std::fs`. |

If TUI-rendered text changes, add or update `insta` snapshots in `codex-tui`.

## Implementation Steps

| Step | Scope | Files |
| --- | --- | --- |
| 1 | Isolate pure logic | `core/src/tools/handlers/hashline_*.rs` first; extract `codex-rs/hashline` if the logic becomes reusable across surfaces |
| 2 | Add read formatter and tests | `codex-rs/hashline/src/read.rs`, `read_tests.rs` |
| 3 | Add parser/apply subset and tests | `codex-rs/hashline/src/parser.rs`, `apply.rs`, sibling tests |
| 4 | Add Codex tool specs/handlers | `core/src/tools/handlers/hashline*.rs`, `handlers/mod.rs`, `spec_plan.rs` |
| 5 | Add runtime approval/sandbox | `core/src/tools/runtimes/hashline.rs`, integration tests |
| 6 | Add code-mode/tool-search coverage | `core/src/tools/spec_plan_tests.rs`, tool definition tests as needed |
| 7 | Decide visibility gate | feature/config/model capability plus schema update if config changes |
| 8 | Evaluate replacement | compare Hashline vs `apply_patch` usage, failure recovery, and test parity |

## Decisions and Residual Risks

1. Line anchors are fixed-width 4-hex values; file and block guards are fixed-width 8-hex
   XXH3-derived compact version guards. They are not cryptographic or adversarial
   collision-resistant boundaries; callers must treat stale-write rejection as best-effort
   protection for trusted workspaces.
2. File guards hash canonical logical text: BOM and CRLF/LF/CR spelling are normalized.
   This protects logical content, not raw-byte representation; exact line terminators
   are still preserved through mutation and new lines inherit a local/default ending.
3. Block selection is heuristic. `find_block` returns the span, excerpt, file guard, and
   block anchor; callers must review that evidence before destructive block edits.
4. Hashline validation and apply_patch handoff remain non-atomic. Post-write verification
   is mandatory, and a failure reports that a mutation may already have occurred.
5. Full-workspace testing and cache hit-rate/cost telemetry are not claims of this change.

## Non-Goals

1. Do not change `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` or
   `CODEX_SANDBOX_ENV_VAR` behavior.
2. Do not add reusable public Hashline APIs to `codex-core`; keep interim
   handler-private modules private until a dedicated crate is justified.
3. Do not make Hashline MCP the default edit path.
4. Do not remove or hide `apply_patch` until replacement criteria are met.
5. Do not emit full-file contents without hard caps.
6. Do not copy reference code that uses direct `std::fs` into native Codex
   runtimes.
