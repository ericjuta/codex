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

Implementation that still remains after the config-first parity work is tracked
separately in:

- [`agentmemory_runtime_expansion_followup_spec.md`](./agentmemory_runtime_expansion_followup_spec.md)

Retrieval-quality and context-injection optimization beyond the parity lane is
tracked separately in:

- [`agentmemory_context_optimization_proposal.md`](./agentmemory_context_optimization_proposal.md)
- [`agentmemory_retrieval_trace_followup_spec.md`](./agentmemory_retrieval_trace_followup_spec.md)

It does not replace the broader architecture and hook-contract decisions in:

- `/Users/ericjuta/Projects/codex/docs/fork-intent.md`
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
3. The human gets an explicit TUI memory control plane for:
   - recall
   - remember
   - knowledge review (`lessons`, `crystals`, `insights`)
   - action orchestration
4. The assistant gets first-class internal memory tools for:
   - recall
   - remember
   - read-only knowledge access
   - action-aware coordination when enabled
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
  - `tui/src/app/app_server_adapter.rs`

Design rule:

- do not create separate retrieval implementations for:
  - slash-command recall
  - assistant-facing recall tool
  - startup retrieval

Instead, create or retain one small shared core helper and have all public
surfaces call that helper.

## Operator Configuration

For Claude-parity hook injection, the primary operator surface is
`~/.codex/config.toml`.

Required settings:

- `[memories]`
  - `backend = "agentmemory"`
- `[memories.agentmemory]`
  - `base_url`
  - `inject_context`
  - `secret_env_var`

Claude-compatible environment variables remain supported as overrides:

- `AGENTMEMORY_URL`
- `AGENTMEMORY_SECRET`
- `AGENTMEMORY_INJECT_CONTEXT`

Design rule:

- persistent setup belongs in `config.toml`
- env vars remain compatibility and override inputs
- the assistant-facing `memory_recall` tool is complementary to hook
  injection, not a replacement for it

## Session Lifecycle

When `config.memories.backend == Agentmemory`, Codex must register session
lifecycle with the backend.

Required calls:

- on session initialization:
  - POST `/agentmemory/session/start`
- during prompt submission:
  - POST `/agentmemory/observe`
  - POST `/agentmemory/context/refresh` on every non-trivial user turn when
    automatic injection is enabled
  - POST `/agentmemory/context` when `context/refresh` is skipped, empty, or
    errors and query-aware retrieval is still warranted
- on session shutdown:
  - POST `/agentmemory/observe`
  - POST `/agentmemory/summarize`
  - POST `/agentmemory/session/end`
  - when consolidation is enabled:
    - POST `/agentmemory/crystals/auto`
    - POST `/agentmemory/consolidate-pipeline`

Required behavior:

- session lifecycle transport must be best-effort but observable in tests
- failure to reach the backend must not crash the main session
- summarize must happen before session end
- bare shutdown `Stop` payloads with only `session_id` / `cwd` must be
  sender-classified as `diagnostics_only`
- `SessionEnd` observe payloads should include summarize outcome when available
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

## Hook Injection Parity

When `config.memories.backend == Agentmemory`, Claude-style hook injection is
separate from the assistant-facing `memory_recall` tool.

Required behavior:

- startup injection comes from `POST /agentmemory/session/start`
- pre-tool enrichment comes from `POST /agentmemory/enrich`
- leaving injection disabled keeps both startup injection and pre-tool
  enrichment off
- `Feature::MemoryTool` continues to gate `memory_recall`, but does not disable
  the hook-based injection lane

## Runtime Surfaces

### Human Surface

Keep these slash commands:

- `/memory-recall [query]`
- `/memory-remember [content]`
- `/memory-update`
- `/memory-drop`

They remain explicit human controls.

Current branch implementation also exposes these human-visible review and
orchestration commands on the same agentmemory seam:

- `/memory-lessons [query]`
- `/memory-crystals`
- `/memory-crystals-create <action_id[,action_id...]>`
- `/memory-crystals-auto [older_than_days]`
- `/memory-insights [query]`
- `/memory-reflect [max_clusters]`
- `/memory-actions [status]`
- `/memory-action-create <title>`
- `/memory-action-update <action_id> <pending|active|done|blocked|cancelled>`
- `/memory-frontier [limit]`
- `/memory-next`

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
- on remember success:
  - show a concrete saved-memory acknowledgement
- on remember failure:
  - show a concrete error event
- on update/drop success:
  - show a concrete completion message, not only a vague "triggered" message
- on recall without a thread:
  - show an explicit thread/session requirement message

### Human Remember Surface

Codex should expose an explicit human-facing durable-write control backed by:

- `POST /agentmemory/remember`

Minimum behavior:

- allow a user to save a durable memory explicitly without relying on passive
  hook capture
- keep the write visible in transcript/history UI
- make success and failure visible to the human

Minimum input shape:

- `content`
- optional workspace/project context when available

Design rule:

- explicit remember writes are a different product surface from passive
  observation capture and should not be hidden inside generic update flows

### Human Knowledge Surfaces

Codex should expose first-class human review surfaces for the derived knowledge
objects already produced by `agentmemory`:

- lessons
- crystals
- insights

Minimum backend operations to support:

- `GET /agentmemory/lessons`
- `POST /agentmemory/lessons/search`
- `GET /agentmemory/crystals`
- `POST /agentmemory/crystals/create`
- `POST /agentmemory/crystals/auto`
- `POST /agentmemory/reflect`
- `GET /agentmemory/insights`
- `POST /agentmemory/insights/search`

The exact UI may be slash commands, dedicated panes, or visible transcript
cells, but the operator must be able to:

- inspect current lessons/crystals/insights
- trigger the distillation paths that produce them when appropriate

### Human Action Surface

Codex should expose a first-class human-facing action surface backed by the
agentmemory orchestration endpoints.

Minimum backend operations to support:

- `GET /agentmemory/actions`
- `POST /agentmemory/actions`
- `POST /agentmemory/actions/update`
- `GET /agentmemory/frontier`
- `GET /agentmemory/next`

The operator should be able to:

- create actions explicitly
- inspect current/open actions
- update action status
- ask the backend for suggested next work

Design rule:

- actions are not observations and not memories; they are explicit work items
  and should appear as such in the runtime UX

### Visual Memory UI

The human-facing memory actions should render as dedicated memory UI cells, not
generic warning/info text.

Minimum visual fields:

- operation:
  - recall
  - remember
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
- operation-specific summary for remember / lessons / crystals / insights /
  actions when those surfaces are invoked

Design rule:

- do not make the human infer memory activity from generic bullets or warning
  styling alone
- memory actions should be visually recognizable at a glance in the transcript

Human-surface principle:

- memory actions must be observable by the human, not only by the assistant.

### Assistant Surface

Add first-class internal memory tools.

Minimum required initial tools:

- `memory_recall`
- `memory_remember`

Current branch implementation also exposes these read-oriented assistant tools:

- `memory_lessons`
- `memory_crystals`
- `memory_insights`
- `memory_actions`
- `memory_frontier`
- `memory_next`

Recommended initial parameters:

- `query: Option<String>`
- `content: String` for `memory_remember`

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
- expose explicit remember writes rather than forcing the assistant to rely only
  on passive observation capture
- do not expose destructive memory-drop behavior to the assistant in this lane

Rationale:

- recall helps the assistant reason
- remember lets the assistant preserve high-value knowledge explicitly
- update remains operational and low-value per turn
- drop is destructive and should remain explicit human control unless policy
  changes later

### Assistant Knowledge Surface

Codex should expose read-oriented assistant tools for the derived knowledge
objects when the backend is `Agentmemory`.

Minimum useful capabilities:

- list/search lessons
- list crystals
- list/search insights

Design rule:

- prefer read-only assistant access for derived knowledge objects
- triggering distillation jobs like `reflect` or `auto-crystallize` may remain
  human-initiated unless and until a stronger product need appears

### Assistant Action Surface

Codex should expose action-aware assistant tools when `agentmemory` is enabled
so the assistant can coordinate around explicit work items rather than only raw
observations.

Minimum useful capabilities:

- list actions
- inspect next/frontier suggestions

Stretch capabilities for the same lane:

- create actions
- update action status

If assistant-side action mutation is enabled, it must remain clearly visible to
the human as structured memory/action UI, not silent background state changes.

## Enablement Gates

The assistant-facing agentmemory tool suite should be exposed only when both
are true:

- `Feature::MemoryTool` is enabled
- `config.memories.backend == Agentmemory`

In wrapped runtimes, "exposed" means callable on the effective model-visible
tool surface, which may be a nested or wrapper-mediated surface rather than the
top-level tool list. Example: a top-level `exec` tool whose callable nested
tools are available via `tools` / `ALL_TOOLS`.

Do not add a new feature flag unless rollout isolation is necessary.

Do not gate the assistant-facing agentmemory tool suite on `memories.use_memories`.

Rationale:

- current `agentmemory` startup behavior already special-cases
  `backend == Agentmemory`
- using `use_memories` here would create inconsistent behavior between startup
  retrieval and mid-session retrieval

Current implementation note:

- `memories.use_memories` is the Codex-native consolidation toggle for the
  session-end `crystals/auto` plus `consolidate-pipeline` side effects
- `CONSOLIDATION_ENABLED=true|false` remains honored as an override so Codex
  can match the standalone `agentmemory` hook runtime when operators already
  use that environment variable

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

The same principle applies to:

- explicit remember writes
- lessons/crystals/insights review
- action orchestration

There should not be one "human-only real path" and one "assistant-only fake
path" for those surfaces.

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

In addition, the lane requires the current plugin-aligned side effects attached
to some of those hooks:

- `UserPromptSubmit`
  - must retain prompt capture via `observe`
  - must use `context/refresh` on every non-trivial user turn when automatic
    injection is enabled
  - must fall back to `context` when `context/refresh` returns no usable
    context
- `Stop`
  - must retain `observe`
  - must also call `summarize`
- `SessionEnd`
  - must call `session/end`
  - when consolidation is enabled, must also call `crystals/auto` and
    `consolidate-pipeline`

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
- sender metadata:
  - `source`
  - `payload_version`
  - `event_id`
  - `capabilities`
- explicit `persistence_class`
- original hook payload data

Required normalization for native post-tool capture:

- `PostToolUse` must emit `tool_input` and `tool_output`
- `PostToolUseFailure` must emit `tool_input` and `error`
- the sender must reject unknown native hook types instead of silently passing
  them through as ad hoc hook names

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

- the standalone `tui`
- the app-server-backed TUI mode routed through `tui/src/app/app_server_adapter.rs`

Those two runtime paths must stay behaviorally aligned.

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
2. Add explicit remember surface on the same backend seam.
3. Add read-oriented lessons/crystals/insights surfaces.
4. Add action list/frontier/next visibility, then optional action mutation.
5. Factor or confirm one shared core retrieval/write path backed by
   `AgentmemoryAdapter`.
6. Add the assistant-facing `memory_recall` and `memory_remember` tools.
7. Align runtime instructions and tool-surface documentation with reality.
8. Add focused tests for:
   - human submit feedback
   - human success/empty/error rendering
   - human inline-query recall rendering
   - explicit remember writes
   - lessons/crystals/insights surfaces
   - action list/frontier/next surfaces
   - assistant tool exposure gates
   - assistant tool recall output
   - assistant remember output
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
- a human can explicitly save durable memory with a visible remember surface
- a human can inspect lessons, crystals, and insights without leaving Codex
- a human can view and manage actions/frontier/next work using the backend
- `/memory-update` and `/memory-drop` visibly acknowledge both submission and
  completion
- the assistant can call a first-class internal recall tool when memory is
  enabled for `agentmemory`
- the assistant can explicitly remember durable knowledge when memory is
  enabled for `agentmemory`
- the assistant can access read-oriented lessons/crystals/insights surfaces
- the assistant can access at least action list/frontier/next surfaces
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
  add dedicated memory-recall / memory-remember / knowledge / action handler
  modules as needed
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
