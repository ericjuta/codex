# Agentmemory Runtime Surface Spec

## Status

Proposed implementation handoff for the runtime-surface lane.

This document is intentionally narrow. It is the canonical handoff for how
`agentmemory` should appear at runtime in this fork.

It does not replace the broader architecture decisions in:

- `docs/agentmemory_payload_quality_spec.md`
- `/Users/ericjuta/Projects/codex/docs/agentmemory-codex-memory-replacement-spec.md`

Those documents answer whether `agentmemory` should be the primary memory
engine and how capture/retrieval quality should work. This document answers the
next question:

- what human-facing and assistant-facing runtime surfaces should exist,
- what they should call internally,
- what should be visible in the TUI,
- what should not be built.

## Goal

Present one coherent memory system to both:

- the human user in the TUI, and
- the assistant at runtime.

The design must avoid hidden memory mutations, duplicate retrieval paths, and
assistant confusion about whether memory is actually available.

## Current Problem

The fork currently has an awkward split:

- the human can explicitly trigger `/memory-recall`, `/memory-update`, and
  `/memory-drop` from the TUI,
- core already knows how to call the `agentmemory` adapter,
- the assistant often cannot see an equivalent first-class callable memory
  surface,
- successful recall can inject developer instructions into the thread without
  giving the human a strong visible explanation of what happened.

That shape is product-incoherent. It causes both:

- user confusion: "I pressed Enter and nothing happened"
- assistant confusion: "memory is not available here" even though the human
  command exists and the backend is healthy

## Decision

The correct runtime design for this fork is:

1. `agentmemory` is the one authoritative runtime memory backend.
2. There is one canonical core recall/update/drop implementation.
3. The human gets an explicit TUI slash-command control plane.
4. The assistant gets a first-class internal recall tool.
5. Both surfaces reuse the same core semantics and backend adapter.
6. MCP is not part of this lane.

## Canonical Core Path

All runtime memory retrieval in this fork should route through one shared core
path backed by `AgentmemoryAdapter`.

Minimum shared inputs:

- `session_id`
- `project` / `cwd`
- `query: Option<String>`
- internal token budget

Current relevant implementation points:

- adapter transport and endpoint selection:
  - `core/src/agentmemory/mod.rs`
- current slash-command recall implementation:
  - `core/src/codex.rs`
- current TUI slash-command dispatch:
  - `tui/src/chatwidget.rs`
  - `tui_app_server/src/chatwidget.rs`

Design rule:

- do not create separate retrieval implementations for:
  - slash-command recall
  - assistant-facing recall tool
  - startup retrieval

Instead, create or retain one small shared core helper and have all public
surfaces call that helper.

## Runtime Surfaces

### Human Surface

Keep these slash commands:

- `/memory-recall [query]`
- `/memory-update`
- `/memory-drop`

They remain explicit human controls.

Required UX behavior:

- on submit:
  - show immediate local feedback in history so the UI never feels inert
- on recall success:
  - show that memory was recalled
  - show the recalled context itself, or a faithful preview of it
  - make it clear that the context was injected into the active thread
- on recall empty result:
  - show an explicit "no relevant memory context found" message
- on recall error:
  - show an error event
- on update/drop success:
  - show a concrete completion message, not only a vague "triggered" message
- on recall without a thread:
  - show an explicit thread/session requirement message

Human-surface principle:

- memory actions must be observable by the human, not only by the assistant.

### Assistant Surface

Add one first-class internal tool for recall.

Recommended initial tool:

- `memory_recall`

Recommended initial parameters:

- `query: Option<String>`

Recommended initial output:

- structured output containing recalled context and whether anything was found

Example shape:

```json
{
  "recalled": true,
  "context": "<agentmemory-context ...>...</agentmemory-context>"
}
```

If nothing is found:

```json
{
  "recalled": false,
  "context": ""
}
```

Design rule:

- expose recall to the assistant first
- do not expose destructive memory-drop behavior to the assistant in this lane
- do not expose memory-update to the assistant unless a concrete product need
  emerges later

Rationale:

- recall helps the assistant reason
- update is operational and low-value per turn
- drop is destructive and should remain explicit human control unless policy
  changes later

## Enablement Gates

The assistant-facing recall tool should be exposed only when both are true:

- `Feature::MemoryTool` is enabled
- `config.memories.backend == Agentmemory`

Do not add a new feature flag unless rollout isolation is necessary.

Do not gate the assistant-facing recall tool on `memories.use_memories`.

Rationale:

- current `agentmemory` startup behavior already special-cases
  `backend == Agentmemory`
- using `use_memories` here would create inconsistent behavior between startup
  retrieval and mid-session retrieval

## Tool/Slash Semantics

The slash command and assistant tool should share the same backend semantics:

- same backend
- same query behavior
- same token-budget policy
- same session scoping

They should differ only in presentation:

- slash command:
  - inject recalled context into the active conversation
  - show user-visible history output
- assistant tool:
  - return recalled context to the assistant as tool output
  - let the assistant decide how to use it in the current turn

This means the human and assistant surfaces are parallel views over one memory
engine, not separate systems.

## TUI Requirements

The TUI must treat memory commands as visible product actions, not invisible
internal mutations.

Required behavior:

- immediate submit acknowledgment in history
- visible completion/result message in history
- no success path that only changes assistant context silently

This applies to both:

- `tui`
- `tui_app_server`

The two implementations must stay behaviorally aligned.

## Documentation Requirements

Once the assistant-facing tool exists, documentation and runtime instructions
must stop implying a tool exists when none is callable.

Required follow-up:

- align developer/runtime instructions with the actual callable tool surface
- avoid telling the assistant to use "AgentMemory tools" unless such tools are
  actually present in the current runtime

## Non-Goals

This lane does not include:

- MCP exposure
- a second memory backend
- hidden auto-recall on arbitrary turns
- assistant-facing memory-drop
- assistant-facing memory-update by default
- reintroducing static `MEMORY.md`-style loading on top of `agentmemory`

## Rollout Order

1. Stabilize human-visible slash-command behavior for recall/update/drop.
2. Factor or confirm one shared core recall path backed by `AgentmemoryAdapter`.
3. Add the assistant-facing `memory_recall` internal tool.
4. Align runtime instructions and tool-surface documentation with reality.
5. Add focused tests for:
   - human submit feedback
   - human success/empty/error rendering
   - assistant tool exposure gates
   - assistant tool recall output

## Acceptance Criteria

The lane is done when all of the following are true:

- a human pressing Enter on `/memory-recall` sees immediate feedback in the TUI
- a human pressing Enter on successful `/memory-recall` sees recalled context in
  the TUI history
- `/memory-update` and `/memory-drop` visibly acknowledge both submission and
  completion
- the assistant can call a first-class internal recall tool when memory is
  enabled for `agentmemory`
- the assistant no longer has to infer memory availability from unrelated MCP
  surfaces
- there is one canonical core recall path, not parallel retrieval
  implementations

## File Plan

Expected primary files for this lane:

- `core/src/agentmemory/mod.rs`
- `core/src/codex.rs`
- `core/src/tools/spec.rs`
- `core/src/tools/handlers/mod.rs`
- `core/src/tools/handlers/`:
  add a dedicated memory-recall handler module
- `core/src/tools/spec_tests.rs`
- `tui/src/chatwidget.rs`
- `tui/src/chatwidget/tests.rs`
- `tui_app_server/src/chatwidget.rs`
- `tui_app_server/src/chatwidget/tests.rs`

## Practical Recommendation

Do not rush into auto-recall heuristics or extra protocols.

Build the lane in this order:

- make the human-visible slash-command path truthful and obvious
- expose one assistant-facing recall tool on top of the same core path
- only then evaluate whether proactive or automatic recall behavior is worth
  adding

That preserves a coherent product model:

- one memory engine
- one core implementation
- two explicit runtime surfaces
- zero MCP dependency for this lane
