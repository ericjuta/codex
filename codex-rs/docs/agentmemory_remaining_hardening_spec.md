# Agentmemory Remaining Hardening Spec

## Status

Pending after the April 20, 2026 feature-complete lane.

This document tracks what remains after the core `agentmemory` integration is
already working in `codex`.

It is intentionally about hardening, confidence, and polish, not missing
baseline functionality.

## Why This Exists

The current branch already ships:

- backend parity for the targeted `agentmemory` surfaces
- assistant tools for the new review lanes
- human slash-command access for the same lanes
- replay/history support for the new memory operations
- packet-backed resume integration
- richer TUI rendering for key review surfaces

That means the product lane is already landed.

What remains is the usual post-feature work:

- broader validation
- hygiene and lint cleanup
- optional UX polish where the current implementation is correct but still
  generic

## Goal

Make the `agentmemory` integration in `codex` feel fully hardened for
long-running daily use without reopening the completed architecture lane.

## Non-Goals

This spec does **not** re-open:

- the mission/handoff core integration
- the runtime expansion baseline
- the slash-command usage guidance
- the backend data model in `agentmemory`

Those are already covered elsewhere.

## Existing Completed Specs

Completed functionality is already tracked in:

- [`agentmemory_runtime_expansion_followup_spec.md`](./agentmemory_runtime_expansion_followup_spec.md)
- [`agentmemory_mission_handoff_followup_spec.md`](./agentmemory_mission_handoff_followup_spec.md)
- [`agentmemory_slash_command_usage_spec.md`](./agentmemory_slash_command_usage_spec.md)

This document starts where those end.

## Remaining Tracks

### Track 1: Broader Validation

Goal:

- move from targeted confidence to branch-level confidence

What remains:

- run the larger `codex-rs` validation slices that were intentionally skipped
  during feature work
- identify whether any failures are caused by the `agentmemory` lane versus
  unrelated repo instability
- document which validation commands are considered the recommended confidence
  bar for this integration branch

Expected outputs:

- a short validation matrix for:
  - `codex-core`
  - `codex-tui`
  - `codex-app-server-protocol`
  - any relevant end-to-end or replay suites
- clear notation of which failures are pre-existing, if any

Notes:

- this is a confidence track, not a feature track
- workspace-wide `cargo test` remains a separate approval and runtime decision

### Track 2: Lint And Hygiene Cleanup

Goal:

- remove residual polish debt left behind by the fast feature lane

What remains:

- scoped `clippy --fix` / `just fix -p ...` passes for touched crates
- any follow-up formatting or minor API-shape cleanup needed after the fix pass
- removal of obviously stale code paths or redundant glue introduced during the
  rapid integration cycle

Expected outputs:

- touched crates are clean under the repo’s normal lint flow
- no known avoidable warnings in the integration path beyond unrelated
  pre-existing ones

Notes:

- keep this scoped
- do not treat unrelated workspace lint drift as part of the agentmemory lane

### Track 3: TUI Ergonomics Polish

Goal:

- make the new memory review surfaces feel purpose-built rather than merely
  technically available

What remains:

- richer presentation for structured memory review payloads where the current
  rendering is still generic
- selective improvements to readability, grouping, and skimmability of:
  - guardrails
  - decisions
  - dossiers
  - handoffs
  - routine candidates
- additional snapshot coverage for the improved rendering states

Expected outputs:

- high-signal rendering of structured payloads without raw JSON leakage for the
  common happy path
- snapshots that clearly lock the intended display format

Notes:

- avoid turning memory history into a second dashboard
- keep the transcript compact and useful

### Track 4: Resume UX Tightening

Goal:

- make packet-backed resume behavior feel more explicit and reliable for human
  operators

What remains:

- verify that resume always surfaces the most relevant session-scoped handoff
  packet at the right time
- evaluate whether the current post-resume handoff review should be made more
  visible or more user-controllable
- ensure packet-first resume semantics remain consistent across:
  - slash-command review
  - resumed app-server threads
  - replayed history

Expected outputs:

- a clearly defined “packet-first resume” UX contract
- tests for the resume path where packet review should appear automatically

Notes:

- do not auto-inject arbitrary handoff content into every prompt
- keep the distinction between human review surfaces and model prompt surfaces

### Track 5: Operator Documentation Cleanup

Goal:

- keep the human-facing docs aligned with the actual runtime behavior

What remains:

- ensure the slash-command usage doc stays aligned with current command
  semantics
- ensure the runtime and mission/handoff docs point cleanly to the remaining
  hardening spec rather than implying nothing is left
- keep the doc graph understandable: implemented lane vs remaining lane

Expected outputs:

- no ambiguity about what is complete vs what is still polish/hardening

## File Plan

Expected primary files:

- `codex-rs/core/src/codex.rs`
- `codex-rs/core/src/codex/agentmemory_ops.rs`
- `codex-rs/core/src/tools/handlers/memory_runtime.rs`
- `codex-rs/tui/src/chatwidget.rs`
- `codex-rs/tui/src/history_cell.rs`
- `codex-rs/tui/src/app.rs`
- `codex-rs/tui/src/chatwidget/tests/`
- `codex-rs/core/tests/`
- `codex-rs/docs/agentmemory_*.md`

## Recommended Order

1. Finish scoped lint/hygiene cleanup.
2. Run the broader validation matrix and record results.
3. Tighten any remaining TUI rendering rough edges.
4. Tighten resume UX only if the broader validation shows gaps or operator
   confusion remains.
5. Close doc drift after the above stabilizes.

## Acceptance Criteria

This hardening spec is complete when:

- the touched crates pass their intended scoped lint flow
- the branch has a documented confidence bar beyond the narrow smoke tests
- the new memory review surfaces render cleanly and predictably in the TUI
- packet-first resume behavior is tested and documented clearly
- the docs distinguish implemented functionality from remaining polish work
