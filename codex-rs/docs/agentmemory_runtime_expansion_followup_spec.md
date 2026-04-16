# Agentmemory Runtime Expansion Follow-Up Spec

## Status

Remaining implementation contract after the config-first hook parity lane.

This document exists so the branch has one focused place to track what is
still missing from the expanded runtime-surface plan without mixing
already-implemented parity work with unimplemented product expansion.

## Baseline

The branch already implements the following baseline:

- config-first `agentmemory` hook parity
- startup injection from `POST /agentmemory/session/start`
- pre-tool enrichment from `POST /agentmemory/enrich`
- visible human surfaces for:
  - recall
  - update
  - drop
- assistant-facing `memory_recall`

That work is tracked by:

- [`agentmemory_runtime_surface_spec.md`](./agentmemory_runtime_surface_spec.md)
- [`../../docs/claude-code-hooks-parity.md`](../../docs/claude-code-hooks-parity.md)

This follow-up spec covers what remains after that baseline.

## Goal

Finish the runtime expansion promised by the fork docs so `agentmemory`
surfaces are coherent across:

- the standalone TUI
- the app-server-backed TUI mode
- assistant-visible tools
- replay/resume history

## In Scope

### 1. Explicit Remember

Add first-class explicit durable-write surfaces for both:

- human: `/memory-remember [content]`
- assistant: `memory_remember`

Required backend operation:

- `POST /agentmemory/remember`

Required behavior:

- remember is distinct from update/consolidation
- success and failure are visible to the human
- assistant remember writes emit visible memory/action history rather than
  silent background state

### 2. Knowledge Review Surfaces

Add human-visible and assistant-readable surfaces for:

- lessons
- crystals
- insights

Minimum backend operations:

- `GET /agentmemory/lessons`
- `POST /agentmemory/lessons/search`
- `GET /agentmemory/crystals`
- `POST /agentmemory/crystals/create`
- `POST /agentmemory/crystals/auto`
- `POST /agentmemory/reflect`
- `GET /agentmemory/insights`
- `POST /agentmemory/insights/search`

Assistant scope for this follow-up:

- read-oriented access only
- do not require assistant-side mutation for derived knowledge in this lane

### 3. Action Surfaces

Add first-class work-item surfaces for:

- actions list
- frontier suggestions
- next suggestions

Minimum backend operations:

- `GET /agentmemory/actions`
- `POST /agentmemory/actions`
- `POST /agentmemory/actions/update`
- `GET /agentmemory/frontier`
- `GET /agentmemory/next`

Recommended rollout split:

1. Human-visible list/frontier/next review
2. Assistant-readable list/frontier/next tools
3. Human action creation/update
4. Optional assistant action mutation

### 4. Lifecycle Expansion

Add the remaining plugin-aligned lifecycle calls that are still absent from the
runtime:

- `POST /agentmemory/context/refresh` when query-aware prompt refresh applies
- `POST /agentmemory/crystals/auto` during session shutdown when enabled
- `POST /agentmemory/consolidate-pipeline` during session shutdown when enabled

Design rule:

- these side effects must stay best-effort and testable
- failures must not break the main Codex turn

### 5. Protocol And Replay Parity

Expand protocol and replay support beyond `Recall|Update|Drop`.

Required additions:

- new memory operation kinds for:
  - remember
  - knowledge review operations as needed
  - action operations as needed
- app-server protocol mappings for those new kinds
- replay/resume preservation for those new history items

## Out Of Scope

This follow-up does not require:

- a second memory backend
- assistant-facing destructive drop
- hidden automatic speculative recall on arbitrary turns
- MCP-specific memory exposure

## File Plan

Expected primary files:

- `core/src/agentmemory/mod.rs`
- `core/src/codex.rs`
- `core/src/tools/spec.rs`
- `core/src/tools/handlers/`
- `protocol/src/items.rs`
- `app-server-protocol/src/protocol/v2.rs`
- `app-server-protocol/src/protocol/thread_history.rs`
- `tui/src/chatwidget.rs`
- `tui/src/chatwidget/slash_dispatch.rs`
- `tui/src/history_cell.rs`
- `tui/src/app/app_server_adapter.rs`

## Acceptance Criteria

This follow-up is complete only when all of the following are true:

- a human can explicitly save durable memory with a visible remember surface
- the assistant can explicitly remember durable knowledge with `memory_remember`
- a human can inspect lessons, crystals, and insights without leaving Codex
- the assistant can read lessons/crystals/insights from first-class tools
- a human can inspect actions/frontier/next work from Codex
- the assistant can access at least read-only actions/frontier/next surfaces
- lifecycle expansion calls are wired for the required cases
- new memory/action UI survives replay and resume as structured memory history
- the standalone TUI and app-server-backed TUI stay behaviorally aligned

## Recommended Order

1. Add `remember` protocol/runtime/UI/tool support.
2. Add read-only lessons/crystals/insights surfaces.
3. Add read-only actions/frontier/next surfaces.
4. Add human action mutation.
5. Add lifecycle expansion calls.
6. Add any remaining replay/app-server follow-through.
