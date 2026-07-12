# Harden Hashline Integrity and Prompt-Cache Efficiency

- Branch: main
- Status: In Progress
- Owner(s): Codex
- Created: 2026-07-12
- Last Updated: 2026-07-12
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
context and that unstable tool-spec ordering can reduce provider prompt-cache
reuse.

After this work, a stale Hashline edit should fail with actionable evidence,
multi-line and block operations should carry a file/block guard, block selection
should be visible and reproducible, output should remain byte-bounded without
duplicating line content, and normal line-ending/path edge cases should preserve
the intended file. The model-visible tool list should be deterministically ordered
so equivalent turns have a stable serialized tool prefix.

Success means:

- focused Hashline tests prove collision-resistant anchors, interior range guards,
  block guards, normalization behavior, parser/path handling, and stale handoff
  behavior;
- serialized Hashline responses remain within their byte budgets while removing
  duplicate line payloads and avoiding an unconditional 200-line write reread;
- prompt-cache observation records a stable final tool-schema digest and tests show
  deterministic model-visible tool ordering;
- no default prompt-cache key, user intent, strategy behavior, or unrelated dirty
  work is changed;
- the spec, implementation commits, final diff, and validation results describe
  residual risks explicitly.

## Progress

- [x] (2026-07-12 15:18Z) Research current behavior, audit proposal, existing prompt-cache handoff, and applicable AGENTS.md policy.
- [ ] (2026-07-12 15:18Z) Define implementation approach and validation plan in this living spec.
- [ ] (2026-07-12) Implement stronger line/file/range/block integrity guards.
- [ ] (2026-07-12) Implement normalization, concurrency handoff, and parser/path safety fixes.
- [ ] (2026-07-12) Remove redundant output and make write responses targeted and compact.
- [ ] (2026-07-12) Canonicalize final model-visible tool ordering without changing cache keys.
- [ ] (2026-07-12) Run focused validation, inspect the final diff, and record outcomes and residual risks.

## Surprises & Discoveries

- Observation: The audit is a static source audit; its proposal explicitly says tests were not run.
  Evidence: the supplied final audit proposal; live repository inspection is required before treating findings as defects.
- Observation: Hashline currently emits the same line text in both flat `content` and structured `lines`, and write success builds a first-200-line reread.
  Evidence: `codex-rs/core/src/tools/hashline_format.rs` and `codex-rs/core/src/tools/handlers/hashline.rs`.
- Observation: Prompt-cache Stage 1 already records provider cached-token telemetry and tool/context digests.
  Evidence: `codex-rs/core/src/prompt_cache_observation.rs` and `codex-rs/docs/prompt_caching_observability_spec.md`.
- Observation: The existing patch grammar supports optional file headers and validates only line-range boundaries.
  Evidence: `codex-rs/core/src/tools/handlers/hashline_patch.rs`.

## Decision Log

- Decision: Keep the existing Hashline wire envelope but make structured line rows metadata-only when flat content is present.
  Rationale: This removes duplicated prompt tokens while preserving the human-readable anchored excerpt and bounded response fields.
  Date/Author: 2026-07-12 / Codex
- Decision: Strengthen line hashes and file hashes compatibly where possible, and accept legacy anchors only when an explicit compatibility path is safe.
  Rationale: The current two-hex line hash and four-hex file hash have materially high collision risk; safe migration should not silently accept a mismatched strong guard.
  Date/Author: 2026-07-12 / Codex
- Decision: Require a file guard for multi-line and block mutations, and expose the selected block span and guard in find-block output.
  Rationale: Endpoint-only validation cannot detect interior edits, and heuristic block selection must be reviewable before mutation.
  Date/Author: 2026-07-12 / Codex
- Decision: Preserve newline semantics deliberately instead of canonicalizing mixed input based on the first newline.
  Rationale: BOM and CRLF normalization are useful for hashing, but mutation must not unexpectedly rewrite unrelated line endings.
  Date/Author: 2026-07-12 / Codex
- Decision: Do not change the default prompt-cache key or move volatile context.
  Rationale: The prompt-cache handoff requires evidence before cache-key or context-boundary changes; deterministic tool ordering is a smaller, behavior-neutral intervention.
  Date/Author: 2026-07-12 / Codex
- Decision: Land implementation in focused commits and run project-scoped validation before considering a broader suite.
  Rationale: Hashline spans core files with existing dirty-tree risk and prompt-cache changes need independently reviewable proof.
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
5. Add tests for same-prefix/same-suffix range changes, hash collisions within the
   old widths, malformed anchors, and stale block edits.

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
4. Canonicalize final model-visible tool-spec ordering by stable identity and add
   tests around equivalent tool-source orderings.
5. Use the existing prompt-cache observation ledger and provider cached-token fields
   for evidence; do not log raw prompts or alter cache-key semantics.

## Context and Orientation

Hashline code lives under `codex-rs/core/src/tools`:

- `handlers/hashline.rs` owns tool schemas, read/find/write/file-operation handlers,
  response shaping, and the bridge to the repository apply-patch engine.
- `handlers/hashline_hash.rs` owns normalization for hashing and line/file hashes.
- `handlers/hashline_format.rs` owns bounded excerpts and structured response rows.
- `handlers/hashline_block.rs` finds heuristic syntactic/indentation blocks.
- `handlers/hashline_patch.rs` parses and applies anchored operations.
- `hashline_tests.rs` and `core/tests/suite/hashline.rs` cover unit and integration
  behavior.

Prompt-cache observability and tool assembly live under `codex-rs/core/src`:

- `prompt_cache_observation.rs` records stable digests and provider cache telemetry.
- `tools/spec_plan.rs` builds the final model-visible tool specification list.
- `mcp_tool_exposure.rs` and related tool modules contribute externally sourced tools.
- `docs/prompt_caching_observability_spec.md` records the staged evidence plan.

The Hashline read envelope currently has both a flat anchored `content` string and
structured `lines` rows. Hashline patch headers are optional in some operations,
line hashes are short, and multi-line validation checks endpoints rather than every
interior line. Block selection is heuristic and currently does not carry a full
file guard. Hashing canonicalizes BOM/line endings and trims trailing whitespace
for line hashes, while mutation can rewrite line endings.

The repository is Rust. The applicable root `AGENTS.md` requires Hashline for
known-file anchored edits, `just fmt` after Rust changes, `just test` rather
than direct `cargo test`, focused project tests before any broader suite, and
explicit review of the final diff/status. Complete workspace testing requires a
separate user approval under that policy.

## Validation Plan

Run after each coherent Rust milestone:

- `just fmt` from `codex-rs`;
- `just test -p codex-core`;
- `git diff --check`;
- targeted unit/integration tests for changed Hashline and tool-order behavior;
- read the resulting spec and changed source back through bounded Hashline reads;
- inspect `git diff --stat`, `git diff`, and `git status --short --branch`.

Do not run a direct `cargo test`. Do not run the complete workspace suite unless
the user explicitly approves it. If a command is unavailable or too expensive,
record it as skipped rather than weakening the acceptance criteria.

## Outcomes & Retrospective

- Outcome: Initial research and the executable work plan are recorded; implementation is pending.
  Evidence: this spec and commit history through `29650fb886`.
  Remaining: Milestones 1-3, validation, final diff review, and residual-risk update.
