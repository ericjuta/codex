# Agentmemory Mission And Handoff Follow-Up Spec

## Status

Implemented on April 20, 2026.

The backend now exposes durable:

- missions
- mission runs
- handoff packets

Codex now consumes the read/generate slice directly through:

- assistant tools:
  - `memory_missions`
  - `memory_handoffs`
  - `memory_handoff_generate`
- human slash commands:
  - `/memory-missions`
  - `/memory-handoffs`
  - `/memory-handoff-generate`
- structured runtime history via memory operation events
- packet-backed resume retrieval through the backend `mem::context` path

## Goal

Make Codex a strong runtime caller of the new mission and handoff state so the
fork can exploit the backend beyond plain recall and action review.

This follow-up is specifically about:

- durable objective tracking
- packet-backed resume flows
- visible runtime surfaces for humans and the assistant

It is not about re-implementing mission logic inside Codex.

## Backend Baseline

The live `agentmemory` backend now provides:

- `POST /agentmemory/missions`
- `POST /agentmemory/missions/update`
- `GET /agentmemory/missions`
- `GET /agentmemory/missions/:id`
- `POST /agentmemory/handoffs/generate`
- `GET /agentmemory/handoffs`
- `GET /agentmemory/handoffs/:id`

It also now upgrades the existing MCP `session_handoff` prompt to packet-backed
output instead of the older thin session/summary dump.

## Implemented Slice

Codex already knew how to:

- call `agentmemory` for session lifecycle
- auto-inject retrieval context
- expose runtime memory tools for recall/remember/lessons/crystals/insights
- expose action/frontier/next read surfaces

This follow-up added the missing direct consumption of:

- mission containers as durable objectives
- handoff packets as resume artifacts
- explicit packet generation for fresh handoffs
- structured runtime history for those interactions

## Decision

Codex should treat backend missions and handoff packets as runtime surfaces,
not just backend internals.

The next slice should stay narrow:

1. read mission state
2. read or generate handoff packets
3. make those results visible in the existing runtime history and assistant
   tool surfaces

Mutation-heavy mission orchestration can come after the read and resume lane is
solid.

## In Scope

### 1. Assistant Read Surfaces

Add assistant-facing tools for:

- `memory_missions`
- `memory_handoffs`

Minimum behavior:

- mission list/get review for the current project
- handoff list/get review for the current project or scope
- optional packet generation when the caller explicitly asks for a fresh handoff

### 2. Human Runtime Surfaces

Add human-visible Codex surfaces for:

- mission review
- packet-backed handoff review

Acceptable first implementations:

- slash commands
- explicit TUI panels
- structured history items that render clearly in replay/resume

### 3. Packet-Backed Resume

Codex resume flows should prefer durable handoff packets when they are
available.

That does not mean blindly auto-injecting them on every turn.

It does mean:

- explicit human resume surfaces should show the latest packet first
- session handoff output in Codex should stay aligned with packet-backed backend
  output
- later automatic resume work should consume packets before inventing a new
  summary layer

## Out Of Scope

This follow-up does not require:

- automatic mission creation from every user turn
- nested mission hierarchies in Codex
- auto-injecting handoff packets into every prompt
- replacing existing action/frontier/next surfaces

## File Plan

Expected primary files:

- `core/src/agentmemory/mod.rs`
- `core/src/codex/agentmemory_ops.rs`
- `core/src/tools/spec.rs`
- `core/src/tools/handlers/memory_runtime.rs`
- `tui/src/slash_command.rs`
- `tui/src/chatwidget.rs`
- `tui/src/history_cell.rs`
- `app-server-protocol/src/protocol/v2.rs`

## Acceptance Criteria

This follow-up is complete when:

- Codex can read backend mission state for the active project
- Codex can read and generate backend handoff packets
- a human can inspect mission and handoff state without leaving Codex
- the assistant can inspect the same state through first-class tools
- replay and resume keep those interactions visible as structured runtime
  history

Status:

- complete for the narrow read and resume lane above
