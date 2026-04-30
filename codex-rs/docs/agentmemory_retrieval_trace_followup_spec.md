# Agentmemory Retrieval Trace Followup Spec

## Status

Initial codex-side runtime support implemented on 2026-04-20.

This document tracks the fork-side work needed to fully exploit the
`agentmemory` retrieval-trace core added in the `agentmemory` repository.

The backend now emits retrieval explainability from `mem::context` through:

- `POST /agentmemory/context`

Codex should not ignore that output when it is the active caller of that
endpoint.

## Goal

Make Codex a strong caller of `agentmemory` retrieval without re-implementing
backend ranking logic.

Concretely, when `agentmemory` returns retrieval trace metadata, Codex should:

- preserve enough of it to explain why automatic context appeared
- keep the preserved shape compact enough for runtime event streams
- avoid pasting raw trace blobs into the conversation by default

## Problem

Before this follow-up, Codex already did the right runtime call:

- non-trivial user turns used `/agentmemory/context` with prompt-derived
  `query`

But Codex only deserialized:

- `context`
- `skipped`

That meant the new backend retrieval trace was discarded immediately. The fork
had no caller-side visibility into:

- selected vs skipped candidate counts
- lane budgets and lane usage
- the first selected and skipped candidates
- the query terms actually seen by backend retrieval

This made the new backend explainability lane hard to use in practice while
debugging automatic recall quality from Codex.

## Decision

Codex should preserve a compact retrieval-trace summary inside its automatic
memory-operation detail, not the full raw backend payload.

Rationale:

- the backend remains the retrieval engine of record
- the runtime event stream needs explainability, not a second store of all
  backend internals
- full candidate lists are too noisy to emit on every automatic retrieval

## Implemented Slice

This follow-up implements the minimum useful Codex-side surface:

1. parse optional retrieval trace data from:
   - `/agentmemory/context`
2. derive a compact summary containing:
   - `query_terms`
   - `selected_count`
   - `skipped_count`
   - `lane_budgets`
   - `lane_usage`
   - a small preview list of selected candidates
   - a small preview list of skipped candidates
3. attach that summary to automatic `MemoryOperationEvent.detail`
   for user-turn retrieval

Files:

- `core/src/agentmemory/retrieval_trace.rs`
- `core/src/agentmemory/mod.rs`
- `core/src/agentmemory/context_planner.rs`
- `core/src/hook_runtime.rs`
- `core/tests/suite/agentmemory_hook_parity.rs`

## Runtime Contract

When `agentmemory` backend retrieval returns a `trace` object:

- Codex automatic user-turn retrieval must preserve a compact
  `retrieval_trace` summary in `MemoryOperationEvent.detail`
- successful query-aware `/context` injection may include the summary
- empty `/context` results may include the summary so operators can still
  inspect what backend retrieval attempted

When no trace is present:

- behavior must remain unchanged
- Codex must not fail deserialization

## Non-Goals

This slice does not:

- expose raw retrieval trace directly in the assistant conversation
- change `memory_recall` tool output shape
- add a new TUI panel just for retrieval trace
- require startup or pre-tool endpoints to synthesize trace if the backend does
  not provide it

## Next Useful Work

If the fork wants to push the lane further, the next reasonable steps are:

1. add an operator-visible TUI affordance for the latest automatic retrieval
   trace summary
2. optionally expose trace summary on assistant `memory_recall` when a debug or
   explicit explainability mode is requested
3. keep query-aware `/context` trace detail inspectable in runtime events

## Acceptance Criteria

- Codex no longer drops retrieval trace returned by `agentmemory`
- automatic memory-operation detail makes backend retrieval decisions
  inspectable
- the preserved shape stays compact and deterministic
- no new retrieval-ranking logic is added on the Codex side
