# Agentmemory Runtime Surface Spec

## Status

Detailed implementation contract for the runtime-surface lane.

This document is the canonical runtime spec for how `agentmemory` should
appear in this fork. It covers:

- human-facing memory controls
- assistant-facing memory controls
- session lifecycle registration
- hook/observation capture requirements
- replay and resume visibility requirements
- verification required to call the lane complete

It does not replace the broader architecture decisions in:

- `docs/agentmemory_payload_quality_spec.md`
- `/Users/ericjuta/Projects/codex/docs/agentmemory-codex-memory-replacement-spec.md`
- `/Users/ericjuta/Projects/codex/docs/claude-code-hooks-parity.md`

Those documents answer whether `agentmemory` should be the primary memory
engine and how retrieval quality should work. This document answers the next
question:

- what human-facing and assistant-facing runtime surfaces should exist,
- what they should call internally,
- what should be visible in the TUI,
- what session and hook events must be emitted to `agentmemory`,
- what visible state must survive replay and resume,
- what tests and verification count as compliance,
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

## Compliance Rule

This spec is intentionally explicit about what counts as "fully compliant".

If the runtime declares a hook, notification, or memory surface in code or docs,
the spec must do one of the following:

- require it,
- explicitly mark it unsupported for this lane, or
- defer it to another named spec.

Ambiguity is not acceptable.

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

## Session Lifecycle

When `config.memories.backend == Agentmemory`, Codex must register session
lifecycle with the backend.

Required calls:

- on session initialization:
  - POST `/agentmemory/session/start`
- on session shutdown:
  - POST `/agentmemory/summarize`
  - POST `/agentmemory/session/end`

Required behavior:

- session lifecycle transport must be best-effort but observable in tests
- failure to reach the backend must not crash the main session
- summarize must happen before session end
- the request bodies must include enough information for backend session views
  to identify the session and workspace

Minimum request shape:

- session start:
  - `sessionId`
  - `project`
  - `cwd`
- summarize:
  - `sessionId`
- session end:
  - `sessionId`

Design rule:

- session lifecycle registration is part of the runtime contract for
  `agentmemory`, not an optional convenience behavior

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

### Visual Memory UI

The human-facing memory actions should render as dedicated memory UI cells, not
generic warning/info text.

Minimum visual fields:

- operation:
  - recall
  - update
  - drop
- status:
  - pending
  - ready
  - empty
  - error
- query when present
- whether recalled context was injected into the current thread
- a wrapped preview body for recalled context or error detail

Design rule:

- do not make the human infer memory activity from generic bullets or warning
  styling alone
- memory actions should be visually recognizable at a glance in the transcript

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

In wrapped runtimes, "exposed" means callable on the effective model-visible
tool surface, which may be a nested or wrapper-mediated surface rather than the
top-level tool list. Example: a top-level `exec` tool whose callable nested
tools are available via `tools` / `ALL_TOOLS`.

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

## Observation and Hook Capture

This lane is not only about recall. It also defines the minimum observation
surface that must be sent to `agentmemory` so recall/update have durable input
to work with.

### Required Hook Observation Events

When `config.memories.backend == Agentmemory`, Codex must capture these
runtime events to the backend:

- `SessionStart`
- `UserPromptSubmit`
- `PreToolUse`
- `PostToolUse`
- `PostToolUseFailure`
- `Stop`

These are the minimum required events for this lane.

### Declared But Optional Hook Families

If code declares additional hook event names such as:

- `SubagentStart`
- `SubagentStop`
- `Notification`
- `TaskCompleted`
- `SessionEnd`

then the implementation must do one of two things:

- capture them and document their payload shape, or
- explicitly mark them unsupported for this lane in this spec and in any
  relevant docs

Design rule:

- do not leave declared hook families in an ambiguous half-supported state

### Hook Payload Requirements

Observed hook payloads must include enough information for downstream memory
analysis to understand what happened.

Minimum required fields:

- session identifier
- workspace/project identity
- cwd
- timestamp
- normalized hook type
- original hook payload data

### Failure Behavior

Observation failures:

- must not fail the main Codex turn
- must be logged
- must remain testable in isolation

This lane does not require retries, durable local buffering, or exactly-once
delivery.

## TUI Requirements

The TUI must treat memory commands as visible product actions, not invisible
internal mutations.

Required behavior:

- immediate submit acknowledgment in history
- visible completion/result message in history
- dedicated visual memory cells for submit and completion states
- no success path that only changes assistant context silently

This applies to both:

- `tui`
- `tui_app_server`

The two implementations must stay behaviorally aligned.

## Persistence and Replay

Human-visible memory actions are product state, not purely transient status.

Required behavior:

- if a memory action is shown in the live TUI transcript, equivalent memory
  state must be reconstructible after:
  - thread resume
  - app-server thread read/rejoin
  - rollout replay
- replayed history must preserve that the item was a memory action rather than
  collapsing it into a generic warning, info message, or disappearing entirely

Minimum persisted or reconstructible fields:

- operation
- status
- query when present
- summary
- preview/detail body when present
- whether recalled context was injected
- whether the source was human or assistant

Design rule:

- live-only memory UI is not sufficient for this lane
- if a user saw a memory result before exiting, they should be able to see the
  same class of result after resuming the thread

This does not require byte-for-byte visual replay parity, but it does require a
durable semantic representation of the memory action.

## Documentation Requirements

Once the assistant-facing tool exists, documentation and runtime instructions
must stop implying a tool exists when none is callable.

Required follow-up:

- align developer/runtime instructions with the actual callable tool surface,
  including wrapper-mediated nested tool surfaces
- avoid telling the assistant to use "AgentMemory tools" unless such tools are
  actually callable in the current runtime
- avoid describing a top-level wrapper-only tool list as the complete callable
  tool surface
- avoid documenting hook families or lifecycle guarantees that are not actually
  emitted by the runtime

## Non-Goals

This lane does not include:

- MCP exposure
- a second memory backend
- hidden auto-recall on arbitrary turns
- assistant-facing memory-drop
- assistant-facing memory-update by default
- reintroducing static `MEMORY.md`-style loading on top of `agentmemory`
- durable offline retry queues for failed backend observation calls
- heuristic speculative recall on every turn

## Rollout Order

1. Stabilize human-visible slash-command behavior for recall/update/drop.
2. Factor or confirm one shared core recall path backed by `AgentmemoryAdapter`.
3. Add the assistant-facing `memory_recall` internal tool.
4. Align runtime instructions and tool-surface documentation with reality.
5. Add focused tests for:
   - human submit feedback
   - human success/empty/error rendering
   - human inline-query recall rendering
   - assistant tool exposure gates
   - assistant tool recall output
   - session lifecycle start/summarize/end ordering
   - required hook observation capture
   - replay/resume reconstruction of human-visible memory actions
   - app-server memory notification mapping

## Acceptance Criteria

The lane is done when all of the following are true:

- a human pressing Enter on `/memory-recall` sees immediate feedback in the TUI
- a human pressing Enter on successful `/memory-recall` sees recalled context in
  the TUI history
- the recall/update/drop history entries are visually distinct memory cells, not
  generic warnings or info bullets
- `/memory-update` and `/memory-drop` visibly acknowledge both submission and
  completion
- the assistant can call a first-class internal recall tool when memory is
  enabled for `agentmemory`
- the assistant no longer has to infer memory availability from unrelated MCP
  surfaces
- there is one canonical core recall path, not parallel retrieval
  implementations
- session start, summarize, and session end are sent to `agentmemory` in the
  required order
- the required hook observation events are sent to `agentmemory` when the
  backend is `Agentmemory`
- any additional declared hook families are either implemented or explicitly
  marked unsupported in this spec
- human-visible memory actions survive resume/replay as memory actions rather
  than disappearing
- the TUI and app-server/TUI paths remain behaviorally aligned for memory
  operations

## File Plan

Expected primary files for this lane:

- `core/src/agentmemory/mod.rs`
- `core/src/codex.rs`
- `core/src/tools/spec.rs`
- `core/src/tools/handlers/mod.rs`
- `core/src/tools/handlers/`:
  add a dedicated memory-recall handler module
- `core/src/tools/spec_tests.rs`
- `core/tests/suite/agentmemory_session_lifecycle.rs`
- `core/src/hook_runtime.rs`
- `tui/src/chatwidget.rs`
- `tui/src/history_cell.rs`
- `tui/src/chatwidget/tests.rs`
- `tui/src/app/app_server_adapter.rs`
- `app-server/src/bespoke_event_handling.rs`
- `app-server-protocol/src/protocol/thread_history.rs`
- `rollout/src/policy.rs`
- `app-server/README.md`

## Verification Matrix

Minimum verification for changes in this lane:

- targeted core tests for:
  - session lifecycle
  - update empty-result behavior
  - recall semantics
  - hook observation capture where applicable
- targeted protocol/tool-spec tests for:
  - `memory_recall` exposure gating
  - `memory_recall` schema and output shape
- targeted TUI tests for:
  - slash-command submission feedback
  - inline query handling
  - success/empty/error rendering
  - replayed memory transcript rendering when applicable
- targeted app-server tests or schema checks for:
  - `thread/memory/operation` notification shape
  - thread resume/read mapping for memory-visible state

A branch should not be described as fully compliant unless both are true:

- the implementation satisfies the acceptance criteria above
- the targeted verification for the touched surfaces passes

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
