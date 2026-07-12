# Harden Hashline Integrity and Prompt-Cache Efficiency

- Branch: feat/hashline-audit-hardening
- Status: Complete
- Owner(s): Codex
- Created: 2026-07-12
- Last Updated: 2026-07-12 22:00Z

- Links: [Hashline tool integration spec](../codex-rs/docs/hashline_tool_integration_spec.md) | [Prompt caching observability spec](../codex-rs/docs/prompt_caching_observability_spec.md) | [Audit proposal](https://chatgpt.com/s/t_6a53aa9fd5c48191937e16ee580984cb)

This ExecPlan is the single source of truth for the end-to-end implementation of
the Hashline audit proposals in this checkout. It is intentionally non-prescriptive:
it improves stale-write safety, evidence quality, parser robustness, and token
efficiency without changing what the user asked Codex to do or changing prompt-cache
keys based on speculation.

## Purpose / Big Picture

Hashline lets Codex read bounded, hash-anchored file excerpts and apply edits
against those anchors. The audit found that short hashes, partial range checks,
heuristic block selection, duplicated output, newline normalization, and
apply-patch handoff details can make a stale or ambiguous edit appear valid. The
same audit found that repeated structured and flat line content wastes model
context. Prompt-cache changes remain telemetry-gated by the existing observability
plan.

After this work, a stale Hashline edit should fail with actionable evidence,
multi-line and block operations should carry a file/block guard, block selection
should be visible and reproducible, output should remain byte-bounded without
duplicating line content, and normal line-ending/path edge cases should preserve
the intended file. Global model-visible tool ordering remains unchanged until
provider telemetry demonstrates material order-only misses.

Success means:

- focused Hashline tests prove fixed-width compact version guards (not cryptographic collision resistance), interior range guards,
  block guards, normalization behavior, parser/path handling, and stale handoff
  behavior;
- serialized Hashline responses remain within their byte budgets while removing
  duplicate line payloads and avoiding an unconditional 200-line write reread;
- prompt-cache observation retains its stable ordered/set digests and provider
  telemetry without speculative prompt-assembly changes;
- no default prompt-cache key, user intent, strategy behavior, or unrelated dirty
  work is changed;
- the spec, implementation commits, final diff, and validation results describe
  residual risks explicitly.

## Progress

- [x] (2026-07-12 15:18Z) Research current behavior, audit proposal, existing prompt-cache handoff, and applicable AGENTS.md policy.
- [x] (2026-07-12 15:18Z) Define implementation approach and validation plan in this living spec.
- [x] (2026-07-12 16:10Z) Implement stronger fixed-width line/file/range/block integrity guards.
- [x] (2026-07-12 16:18Z) Implement normalization, mixed-newline preservation, and parser/path safety fixes.
- [x] (2026-07-12 16:22Z) Remove redundant structured line content, compact JSON, and target write-success excerpts.
- [x] (2026-07-12 19:52Z) Preserve existing model-visible tool ordering after repository review confirmed canonicalization is telemetry-gated.
- [x] (2026-07-12 21:40Z) Run final format/lint, independent branch review, full scoped validation, and diff/status verification; record final outcomes.

## Surprises & Discoveries

- Observation: The audit is a static source audit; its proposal explicitly says tests were not run.
  Evidence: the supplied final audit proposal; live repository inspection is required before treating findings as defects.
- Observation: Initial Hashline emitted the same line text in both flat content and structured lines, and write success built a first-200-line reread.
  Evidence: codex-rs/core/src/tools/handlers/hashline_format.rs and codex-rs/core/src/tools/handlers/hashline.rs.
- Observation: Prompt-cache Stage 1 already records provider cached-token telemetry and tool/context digests.
  Evidence: codex-rs/core/src/prompt_cache_observation.rs and codex-rs/docs/prompt_caching_observability_spec.md.
- Observation: The initial patch grammar accepted optional file headers and validated only line-range boundaries.
  Evidence: codex-rs/core/src/tools/handlers/hashline_patch.rs.
- Observation: Global tool-order canonicalization changed every request prefix without provider evidence and contradicted the staged prompt-cache observability plan.
  Evidence: repository code review; the ordering implementation and expectation churn were removed while existing digest telemetry remains.
- Observation: Write success used the generic first-200-line read envelope, while patch success already had changed-region preview bounds.
  Evidence: build_hashline_write_output and build_hashline_patch_success_body in codex-rs/core/src/tools/handlers/hashline.rs; write integration coverage now targets lines 201-202.
- Observation: The follow-up audit found pasted read-output parsing still hard-coded to two hex digits.
  Evidence: `strip_read_output_payload_prefix` now uses `LINE_HASH_WIDTH`, and a formatter-to-pasted-payload round-trip test covers the protocol boundary.
- Observation: `find_block` emitted a combined line/block anchor that its resolver did not consume.
  Evidence: the resolver now parses and validates both the line hash and recomputed block hash; round-trip and stale-block tests cover it.
- Observation: Reattaching newline separators by output index changed untouched mixed-EOL boundaries after inserts or deletes.
  Evidence: mutation now carries terminators on source-line records, with cardinality-changing, BOM, and final-newline tests.
- Observation: Deleting the final unterminated logical line could transfer the predecessor's terminator to EOF, turning a no-final-newline file into a newline-terminated file and losing the original local EOL fallback for later tail insertion.
  Evidence: `SourceLine` mutation now transfers the `None` terminator to the new final record and carries the original fallback ending through all operations; regression coverage exercises deletion followed by `INS.TAIL`.


## Decision Log

- Decision: Keep the existing Hashline wire envelope but make structured line rows metadata-only when flat content is present.
  Rationale: This removes duplicated prompt tokens while preserving the human-readable anchored excerpt and bounded response fields.
  Date/Author: 2026-07-12 / Codex
- Decision: Use fixed-width compact line and file guards and reject legacy-width anchors.
- Rationale: This is a greenfield contract; accepting short hashes would preserve the audit's collision risk and create ambiguous stale-write behavior.
- Date/Author: 2026-07-12 / Codex
- Decision: Require a file guard for multi-line and block mutations, and expose the selected block span and guard in find-block output.
  Rationale: Endpoint-only validation cannot detect interior edits, and heuristic block selection must be reviewable before mutation.
  Date/Author: 2026-07-12 / Codex
- Decision: Preserve newline semantics deliberately with source-line records instead of reattaching separators by output index.
  Rationale: Canonical BOM/EOL normalization is useful for hashing, while untouched line records must retain their exact terminators through cardinality-changing edits.
  Date/Author: 2026-07-12 / Codex
- Decision: Do not change the default prompt-cache key, context boundaries, or model-visible tool ordering.
  Rationale: The prompt-cache handoff requires provider evidence before prompt-assembly changes; compact bounded Hashline outputs reduce context pressure independently.
  Date/Author: 2026-07-12 / Codex
- Decision: Land implementation in focused commits and run project-scoped validation before considering a broader suite.
  Rationale: Hashline spans core files with existing dirty-tree risk and prompt-cache changes need independently reviewable proof.
  Date/Author: 2026-07-12 / Codex
- Decision: Keep formatter and parser widths coupled through `LINE_HASH_WIDTH` and test the emitted row format as a round trip.
  Rationale: A fixed-width greenfield grammar should not drift between model-visible output and pasted patch input.
  Date/Author: 2026-07-12 / Codex
- Decision: Parse and validate the complete `find_block` block anchor rather than advertising an output-only token.
  Rationale: A returned block selection must be replayable and must fail when either the anchor line or selected block is stale.
  Date/Author: 2026-07-12 / Codex
- Decision: Treat final-newline state as part of source-line record semantics, including cardinality-changing deletions.
  Rationale: An EOF terminator is not an interchangeable separator; preserving it prevents unrelated representation changes while retaining a stable fallback for newly inserted lines.
  Date/Author: 2026-07-12 / Codex


## Implementation Plan

### Milestone 1: Integrity and stale-write safety

1. Increase the effective strength of line and file guards with explicit width-aware
   parsing and diagnostics.
2. Validate every line in a mutated range, or use a full-file guard where the
   operation format cannot carry all interior anchors.
3. Require and validate file guards for block and multi-line operations.
4. Include the selected block's file/range evidence in find-block output and make
   block mutation reject stale selection evidence.
5. Add tests for interior range changes, malformed anchors, unambiguous paths, and stale block edits.

### Milestone 2: Normalization and parser safety

1. Separate canonical text used for hashing from the exact text representation
   used for mutation.
2. Preserve final-newline and line-ending behavior, including mixed line endings,
   BOM handling, and empty files, with explicit tests.
3. Make file-header parsing unambiguous for paths containing `#` and reject
   ambiguous or malformed headers with actionable errors.
4. Remove payload/header parsing ambiguity for literal lines beginning with
   `#` or `[` where the patch grammar permits it.
5. Narrow the check-to-apply race in the Hashline-to-apply-patch handoff and document
   any remaining non-atomic filesystem limitation.

### Milestone 3: Bounded output and prompt/cache efficiency

1. Keep serialized responses within existing byte budgets while replacing duplicate
   structured line content with compact metadata.
2. Make write success return a bounded excerpt around the changed region rather than
   an unconditional first-200-line reread.
3. Prefer compact JSON serialization for model-facing Hashline responses where
   whitespace has no semantic value.
4. Retain the existing prompt-cache observation ledger, ordered/set digests, and
   provider cached-token fields for evidence; do not log raw prompts, reorder global
   tool specs, or alter cache-key semantics without measured order-only misses.

## Context and Orientation

Hashline code lives under `codex-rs/core/src/tools`:

- `handlers/hashline.rs` owns tool schemas, read/find/write/file-operation handlers,
  response shaping, and the bridge to the repository apply-patch engine.
- `handlers/hashline_hash.rs` owns normalization for hashing and line/file hashes.
- `handlers/hashline_format.rs` owns bounded excerpts and structured response rows.
- `handlers/hashline_block.rs` finds heuristic syntactic/indentation blocks and resolves replayable block anchors.
- `handlers/hashline_patch.rs` applies anchored operations and builds previews/diffs.
- `handlers/hashline_patch_parser.rs` owns operation, payload, range, and anchor grammar.
- `handlers/hashline_patch_sections.rs` owns section/header parsing and validation.
- `handlers/hashline_patch_lines.rs` owns exact source-line terminators and EOL-preserving mutation helpers.
- `hashline_tests.rs` and `core/tests/suite/hashline.rs` cover unit and integration
  behavior.

Prompt-cache observability and tool assembly live under `codex-rs/core/src`:

- `prompt_cache_observation.rs` records stable digests and provider cache telemetry.
- `tools/spec_plan.rs` builds the final model-visible tool specification list.
- `mcp_tool_exposure.rs` and related tool modules contribute externally sourced tools.
- `docs/prompt_caching_observability_spec.md` records the staged evidence plan.

The Hashline read envelope has both a flat anchored content string and metadata-only
structured lines rows. Existing-file patch headers use the canonical [path]#HASH
form, line hashes are fixed-width, and multi-line validation checks both endpoints
plus a file guard. Block selection is heuristic but carries a full file/block guard.
Hashing canonicalizes BOM and line endings while mutation preserves each source-line
record's terminator, including mixed input; inserted lines inherit a local/default ending.


The repository is Rust. The applicable root `AGENTS.md` requires Hashline for
known-file anchored edits, `just fmt` after Rust changes, `just test` rather
than direct `cargo test`, focused project tests before any broader suite, and
explicit review of the final diff/status. Complete workspace testing requires a
separate user approval under that policy.

## Validation Plan

Run after each coherent Rust milestone:

- just fmt from codex-rs;
- just test -p codex-core hashline --no-capture;
- git diff --check;
- targeted unit/integration tests for Hashline protocol round trips and unchanged tool-order behavior;
- read the resulting spec and changed source back through bounded Hashline reads;
- inspect git diff --stat, git diff, and git status --short --branch.


Do not run a direct `cargo test`. Do not run the complete workspace suite unless
the user explicitly approves it. If a command is unavailable or too expensive,
record it as skipped rather than weakening the acceptance criteria.

## Before / After / Net Unlocks

Before this branch, Hashline relied on shorter collision-prone anchors, could miss stale interior range changes, emitted a `find_block` anchor its own resolver could not replay, and could rewrite unrelated BOM/EOL/final-newline representation during cardinality-changing edits. Model-facing output also duplicated line content, write success could reread an arbitrary first 200 lines, and important multi-file atomicity, output-budget, file-operation, and representation-only-write boundaries were not proven end to end.

After this branch, the greenfield protocol uses fixed-width 4-hex line anchors and mandatory 8-hex file/block version guards for existing-file mutations; read and block anchors round-trip; stale range, block, rename/remove, and multi-file operations reject before mutation; exact line terminators survive structural edits; and compact byte-bounded responses avoid duplicated line payloads and oversized no-op output. Prompt-cache keys and global model-visible tool ordering remain unchanged pending provider telemetry.

This net unlocks safer autonomous multi-line, block, and multi-file editing; reliable read-to-edit protocol replay; representation-safe work on mixed-EOL/BOM/no-final-newline files; lower model-context pressure; and clearer maintenance boundaries across parser, section, block, line-ending, and application modules. These are correctness and efficiency guarantees for trusted workspaces, not atomic filesystem transactions, adversarial collision resistance, syntax-aware block selection, or raw-byte representation guards.

## Outcomes & Retrospective

- Outcome: Integrity, normalization, parser, and bounded-output changes are implemented in scoped Hashline/core surfaces; speculative global tool ordering was removed after review.
  Evidence: hashline.rs, hashline_hash.rs, hashline_format.rs, hashline_patch.rs, hashline_patch_lines.rs, and focused tests.
- Outcome: The final focused Hashline target selected 153 tests and all 153 passed; 2 remote-environment tests self-skipped while 2,987 unrelated tests were filtered.
  Evidence: `just test -p codex-core hashline --no-capture` completed after protocol round-trip, atomicity, output-budget, exact-byte, and no-op-write regression coverage was added.
- Outcome: The final full `codex-core` suite ran 3,125 tests: all 3,125 passed, 15 skipped, and 3 unrelated flaky tests passed on retry.
- Outcome: Independent repository review drove removal of global tool sorting, structural extraction, stronger integration coverage, and final no-op/representation-only write-response fixes; dedicated unit and handler-level regressions prove successful compact output for applied and dry-run paths.
- Outcome: The branch is split into focused implementation, audit-follow-up, parser/EOF, review-fix, structural, protocol-test, and final-response-fix commits.
- Residual: Prompt-cache hit-rate/cost telemetry was not remeasured; existing ordered/set digest and provider cache telemetry remain the gate for any future canonicalization.
- Residual: The apply-patch handoff is still not an atomic filesystem transaction; a concurrent external writer can race after validation, so the file guard remains the detection boundary.
- Residual: The 8-hex file/block guards and 4-hex line anchors are compact non-cryptographic checksums, not an adversarial security boundary.
- Residual: File guards cover canonical logical text and intentionally ignore BOM/EOL representation changes; raw-byte identity is not guarded.
- Residual: Block selection remains heuristic; span, excerpt, file guard, and block anchor evidence are returned, but semantic selection still needs caller/model review.
