# Agentmemory Context Optimization Proposal

## Status

Proposal for post-parity retrieval and context-injection optimization.

This document answers a narrower question than the existing runtime-surface
specs:

- when an operator already runs with `agentmemory` enabled,
- are we actually maximizing useful memory/context injection,
- and if not, what should change?

Short answer: no. The current implementation is conservative and coherent
enough for parity, but it is not yet optimized for retrieval yield,
deduplication, or clear persistence semantics.

After verification against the local `agentmemory` repository, one correction
is important up front: `agentmemory` already owns substantial retrieval logic.
Its `mem::context` path already implements:

- hot / warm / cold retrieval lanes
- query-aware lane budgeting
- cross-lane fingerprint deduplication
- session working-set and turn-capsule freshness handling

So the Codex problem is not "missing retrieval architecture" on the backend.
The Codex problem is that it is still a weak caller of that architecture.

This proposal assumes the operator baseline is already:

```toml
[memories]
backend = "agentmemory"

[memories.agentmemory]
inject_context = true
```

## Baseline

Today, with `agentmemory` enabled, Codex behaves like this:

- startup injection comes from `POST /agentmemory/session/start`
- prompt-submit refresh comes from `POST /agentmemory/context/refresh`
- pre-tool enrichment comes from `POST /agentmemory/enrich`
- pre-tool enrichment is limited to `Edit|Write|Read|Glob|Grep`
- human recall injects developer context into thread history
- assistant `memory_recall` returns tool output for the current turn, but does
  not inject that context into thread history

Relevant current implementation lives in:

- `core/src/codex.rs`
- `core/src/hook_runtime.rs`
- `core/src/codex/agentmemory_ops.rs`
- `core/src/tools/handlers/memory_runtime.rs`

That shape is reasonable for a conservative parity lane. It is not a
min-maxed memory system.

## Verified Agentmemory Constraints

The proposal below should align with these existing `agentmemory` realities:

- `mem::context` already budgets hot / warm / cold lanes at 40/30/30 without a
  query and 20/40/40 with a query
- `mem::context` already performs cross-lane deduplication before emitting the
  final `<agentmemory-context ...>` block
- `/agentmemory/context/refresh` is the query-aware re-ranking path, not a
  different memory system
- at least one other integration (`integrations/openclaw`) already falls back
  from `/agentmemory/context/refresh` to `/agentmemory/context`
- `memory_recall` / `mem::search` already support token-budgeted outputs

Design consequence:

- Codex should not implement a second retrieval-ranking engine on top of
  `agentmemory`
- Codex should decide when to retrieve, what query to send, what budget to
  request, and what persistence scope to apply
- `agentmemory` should remain the owner of freshness ranking and intra-response
  deduplication

## Current Problems

### 1. One boolean controls multiple strategies

`inject_context = true` currently means "turn on the parity lane", but it does
not yet deliver the aggressive retrieval behavior desired for always-on
operators.

### 2. Automatic retrieval is fragmented

Startup injection, prompt refresh, and pre-tool enrichment are separate
heuristics rather than one shared planner. That creates inconsistent behavior
and makes it hard to reason about:

- why context appeared,
- why it did not appear,
- and whether the enabled path is actually aggressive enough to match operator
  intent.

### 3. Prompt refresh uses a weak trigger

The current prompt-refresh path is effectively gated by prompt length rather
than by retrieval value. A long but low-information steer can qualify, while a
short but high-signal prompt may not get the right treatment.

### 4. Pre-tool enrichment is too string-list-driven

The current matcher hardcodes tool names instead of classifying tools by
capability and by whether they already expose useful structured
`agentmemory_input`.

### 5. Human and assistant recall have mismatched persistence semantics

Today:

- human recall persists injected developer context into thread history
- assistant recall returns ephemeral tool output for the current turn

That difference may be intentional, but it is not explicit enough. The runtime
should make persistence a first-class choice rather than an accidental
property of which surface was used.

### 6. Budgeting and dedupe are implicit

The current design does not expose one clear per-turn policy for:

- how much memory can be injected,
- how often the same context may be reinjected,
- or why a memory block was skipped.

## Decision

Codex should add a unified `agentmemory` context planner and keep
`inject_context` as the only operator-facing switch for automatic injection.

The planner should decide:

- whether retrieval is warranted,
- which backend endpoint to call,
- what request budget to pass,
- whether the result should be turn-local or thread-persisted,
- and whether an exact duplicate result should be suppressed in the immediate
  runtime window.

This keeps the current backend contract intact while making behavior coherent.
It also avoids duplicating backend-side lane ranking, budgeting, and
deduplication that `agentmemory` already performs.

Enabled-path rule:

- `inject_context = false`
  - no automatic injection
- `inject_context = true`
  - one aggressive automatic policy
  - no additional mode selector

## Goals

- Maximize relevant recall yield when the operator has intentionally enabled
  `agentmemory`.
- Keep token cost explicit, bounded, and testable.
- Reduce duplicate or low-value reinjection.
- Make persistence semantics explicit across human and assistant surfaces.
- Preserve conservative guardrails around shell/exec and other high-noise
  tools.
- Keep the initial implementation compatible with existing `agentmemory`
  endpoints.

## Non-Goals

- Broad automatic enrichment for `shell` or `exec`.
- Unbounded recall on every turn.
- Replacing `agentmemory` with another backend.
- Silent persistence of assistant recall.
- Requiring new backend endpoints in the initial implementation phase.

## What This Unlocks

If Codex adopts this proposal, enabling `inject_context` stops meaning
"conservative parity leftovers are on" and starts meaning "Codex aggressively
uses agentmemory during execution."

Concretely, this unlocks:

- better first-turn startup context when a session begins
- query-aware retrieval on normal user turns without relying on prompt length
  as the gate
- `/context/refresh -> /context` fallback so retrieval does not silently stop
  when refresh returns empty or skipped
- aggressive pre-tool enrichment on every eligible capability-class file/search
  tool turn
- broader use of agentmemory's existing hot / warm / cold retrieval engine
  during real execution, not only during explicit recall
- less split-brain between "memory exists in the backend" and "the model is
  actually seeing memory now"
- explicit reinjection suppression so aggressive automatic retrieval does not
  collapse into blindly pasting the same block over and over
- a clearer product story: `inject_context=true` means one strong automatic
  behavior, not a half-conservative compatibility mode

This is most useful for:

- long debugging sessions
- multi-file refactors
- resumed threads
- repeated search / read / edit / patch loops
- tasks where earlier failures, recent conclusions, and touched files should
  continue shaping the model without manual recap

## Proposed Design

### 1. Keep One Operator Switch

Do not add a policy matrix such as `parity|balanced|aggressive`.

Keep:

- `inject_context = false`
- `inject_context = true`

Meaning:

- `false` disables automatic injection
- `true` enables the single aggressive automatic strategy defined in this
  document

Rationale:

- matches the requested product shape
- avoids a config surface full of vague retrieval personalities
- forces the design discussion onto actual behavior instead of naming modes

### 2. Add a unified context planner

Introduce one internal planner, for example:

- `plan_agentmemory_context(...)`

The planner runs before:

- the first model request in a session
- each new user turn
- qualifying tool continuations
- explicit human/assistant recall injection paths

The planner returns a structured decision:

- `reason`
  - `session_start`
  - `user_turn`
  - `pre_tool`
  - `human_recall`
  - `assistant_recall`
- `query`
- `candidate_endpoint`
  - `session/start`
  - `context/refresh`
  - `enrich`
  - `context`
- `request_budget_tokens`
- `inject_scope`
  - `none`
  - `turn`
  - `thread`
- `reinject_key`
- `skip_reason`, when suppressed

Design rule:

- backend retrieval/ranking stays in `agentmemory`
- caller-side timing, budget selection, scope, and reinjection suppression live
  in the planner

Required enabled-path behavior:

- when `inject_context = true`, every non-trivial user turn must attempt
  retrieval
- "non-trivial user turn" means any user turn with non-empty input that is not
  solely a minimal steer such as:
  - "continue"
  - "ok"
  - "thanks"
  - a single-word acknowledgement with no task content
- the default enabled path must not rely on prompt length as the primary gate
- the runtime may skip only when:
  - the turn is trivial by the rule above
  - exact same context was already injected in the immediate duplicate
    suppression window
  - retrieval failed and fallback also failed

### 3. Align with the agentmemory-native retrieval model

Codex should treat `agentmemory` as the retrieval engine of record.

That means:

- do not recreate hot / warm / cold lane assembly in Codex
- do not recreate query-aware lane splits in Codex
- do not attempt content-level deduplication inside the returned context block

Codex should own only:

- trigger selection
- endpoint selection
- request-budget selection
- explicit persistence scope
- exact-duplicate suppression for already injected context blocks in the
  immediate runtime window

Two layers of dedupe are correct:

- backend dedupe:
  - deduplicate candidate memory blocks within one retrieval result
- Codex dedupe:
  - avoid reinjecting the exact same returned context block inside the same
    request path and across the immediately adjacent turn or two

### 4. Aggressive User-Turn Retrieval

Stop using prompt length and weak heuristic steering as the main retrieval
gate.

Enabled-path rule:

- on every non-trivial user turn, first call `/agentmemory/context/refresh`
  with the full user-turn query
- if `/agentmemory/context/refresh` returns non-empty context, inject it
  immediately into the active turn
- if `/agentmemory/context/refresh` returns `skipped = true`, empty context, or
  no context, then always fall back to `/agentmemory/context`
- if fallback `/agentmemory/context` returns non-empty context, inject it
  immediately into the active turn
- if both calls return no usable context, emit a visible "retrieval attempted
  but empty" result

Design rule:

- retrieve aggressively
- inject aggressively
- persist selectively

The point of the enabled path is maximum runtime context availability, not
minimal token spend or elegant sparsity.

Required enabled-path rule:

- for every non-trivial user turn under `inject_context = true`, Codex must do
  one of:
  - inject non-empty context from `/agentmemory/context/refresh`
  - inject non-empty context from fallback `/agentmemory/context`
  - emit a visible "retrieval attempted but empty" result after both paths
    returned no usable context

### 5. Classify tool enrichment by capability

Stop deciding enrichment solely from a hardcoded tool-name list.

Instead, enrich tools that:

- provide structured `agentmemory_input`
- are classified as one of:
  - `FileRead`
  - `FileSearch`
  - `FileWrite`
  - `Patch`

Keep explicit deny rules for:

- `shell`
- `exec`
- network-mutating tools
- tools with no useful structured retrieval payload

Design rule:

- a new file/search tool should become eligible through capability
  classification, not through adding another string comparison
- when `inject_context = true`, automatically call `/agentmemory/enrich` on
  every eligible capability-class tool turn

Canonical current eligible tool set:

- `Edit`
- `Write`
- `Read`
- `Glob`
- `Grep`
- `apply_patch` paths that map to `Edit` or `Write`
- `list_dir` / directory-enumeration paths that map to `Glob`

If a current Codex tool handler already emits `PreToolUsePayload` with
structured `agentmemory_input` and maps semantically into one of the capability
classes above, it must be treated as eligible even if the user-visible tool
name differs from the internal lane name.

Aggressive pre-tool behavior:

- structured file or term inputs should be forwarded whenever available
- no mode gate or secondary selectiveness gate beyond capability eligibility
- no shell / exec auto-enrichment
- no network-mutating auto-enrichment
- any non-empty enrich result should be injected immediately into the active
  turn
- Codex may still suppress exact repeated reinjection of identical returned
  context within the same request path and across the immediately adjacent turn
  or two

### 6. Make persistence explicit

Define three scopes:

- `none`
  - retrieved but not attached to model context
- `turn`
  - injected as developer context for the active turn only
- `thread`
  - recorded into conversation history

Rules:

- automatic startup / refresh / pre-tool injection uses `turn`
- human `/memory-recall` uses `thread`
- assistant `memory_recall` remains turn-local by default
- assistant-triggered thread persistence should use explicit `scope: "thread"`
  on `memory_recall`
- do not introduce a second assistant recall tool solely to express persistence
  scope

No silent promotion from `turn` to `thread`.

Design rule:

- more persistence is not the same as more usable context
- thread persistence should remain selective
- aggressive automatic retrieval should still prefer turn-local injection by
  default

### 7. Add caller budgets and exact-duplicate suppression

Introduce explicit caps such as:

- `default_context_budget_tokens`
- `query_context_budget_tokens`
- `pretool_context_budget_tokens`
- `max_auto_injections_per_turn`
- `reinject_after_turns`

Planner requirements:

- pass explicit request budgets down to `agentmemory`
- do not re-split hot / warm / cold budgets locally in Codex
- hash the returned context block before injection
- suppress exact duplicate returned context blocks within the same request path
  and across the immediate adjacent-turn window
- do not attempt broad fuzzy semantic suppression in the first implementation
- record budget requested and budget used when the backend returns that data

### 8. Make injection visible

Every inject or skip decision should emit structured state:

- source endpoint
- reason
- query
- request budget
- tokens used, when returned by the backend
- dedupe / reinjection outcome
- scope
- whether content was actually injected

Human-visible surfaces should make it obvious:

- that `agentmemory` injected context for the turn
- why it did so
- whether it persisted to thread history
- whether it skipped due to dedupe or budget
- whether a refresh result fell back to `/context`

### 9. Keep the initial backend contract stable

The first implementation phase should keep using the existing endpoints:

- `POST /agentmemory/session/start`
- `POST /agentmemory/context/refresh`
- `POST /agentmemory/enrich`
- `POST /agentmemory/context`

This proposal does not require a new backend planning endpoint up front.

## Configuration Proposal

Keep the operator surface minimal:

```toml
[memories.agentmemory]
inject_context = true
```

Meaning:

- `inject_context = false`
  - no automatic injection
- `inject_context = true`
  - startup injection enabled
  - query-aware refresh enabled
  - `/context/refresh -> /context` fallback enabled
  - aggressive capability-class pre-tool enrichment enabled

The rest of the behavior in this proposal should be runtime defaults, not a
menu of user-facing policy modes.

If Codex needs numeric tuning such as budgets or reinjection windows, prefer
internal constants first. Add config knobs only if later evidence shows that
operators truly need them.

Default runtime meaning of `inject_context=true`:

- always retrieve on every non-trivial user turn
- always `context/refresh -> context`
- always inject any non-empty result
- always enrich every eligible file/search/write tool turn
- always use `turn` scope for automatic injection
- only persist selectively
- only suppress exact duplicate blocks in the immediate runtime window

## Human / Assistant Surface Alignment

### Human

- `/memory-recall` continues to inject into thread history
- UI should show whether the result was empty, turn-local, or persisted
- replay/resume preserves the operation and its scope

### Assistant

- `memory_recall` should report:
  - whether anything was found
  - what scope was applied
- if scope is `thread`, emit the same visible memory operation event shape used
  by human recall
- human and assistant surfaces should reuse one core helper for recall and
  injection semantics

## Implementation Plan

### Phase 1. Planner and observability

Primary files:

- `core/src/hook_runtime.rs`
- `core/src/codex.rs`
- `core/src/agentmemory/mod.rs`
- `config/src/types.rs`

Deliverables:

- planner output type
- budget/dedupe tracking
- `inject_context=true` maps to the aggressive automatic strategy

### Phase 2. Replace brittle heuristics

Primary files:

- `core/src/hook_runtime.rs`
- `core/src/tools/registry.rs`
- tool handlers that already populate `agentmemory_input`

Deliverables:

- signal-based prompt refresh
- `/context/refresh` to `/context` fallback behavior
- capability-based pre-tool eligibility
- aggressive pre-tool enrichment for every eligible capability-class tool turn
- tests for reinjection suppression and skip reasons

### Phase 3. Align recall semantics

Primary files:

- `core/src/codex/agentmemory_ops.rs`
- `core/src/tools/handlers/memory_runtime.rs`
- protocol/app-server/TUI memory-history surfaces

Deliverables:

- explicit assistant recall scope
- visible replayable thread-injection events
- no ambiguity about persistence

### Phase 4. Tune defaults and docs

Primary files:

- `codex-rs/docs/`
- config schema and config docs
- TUI/app-server visibility docs

Deliverables:

- clear operator guidance
- make it explicit that `inject_context=true` means aggressive automatic
  injection
- full acceptance-test coverage

## Verification

Add or update tests that prove:

- `inject_context=false` disables automatic injection
- `inject_context=true` enables aggressive automatic injection
- enabled injection attempts retrieval on every non-trivial user turn without
  using the prompt-length heuristic as the main gate
- trivial acknowledgements such as `ok`, `thanks`, or bare `continue` may skip
  retrieval, but ordinary task-bearing turns may not
- enabled injection falls back from `/context/refresh` to `/context` when query
  refresh yields no context but retrieval is still warranted
- any non-empty automatic retrieval result is injected into the current turn
- capability-based pre-tool enrichment covers tools with structured
  `agentmemory_input`, not just hardcoded names
- exact duplicate context blocks are suppressed only within the immediate
  runtime window
- every eligible capability-class pre-tool turn attempts enrichment when
  injection is enabled
- human recall persists to thread history with explicit scope metadata
- assistant recall can remain turn-local or explicitly persist to thread via
  `scope: "thread"`
- replay/resume shows why a memory block was injected and under what scope
- caller budgets are passed to `agentmemory` deterministically
- Codex does not duplicate agentmemory-side lane ranking or intra-response
  deduplication logic

## Acceptance Criteria

This proposal is implemented well when all of the following are true:

- users who run with `agentmemory` enabled get more relevant automatic recall
  than the current parity lane
- retrieval is attempted on every non-trivial user turn when injection is
  enabled
- trivial acknowledgements are the only normal user-turn class allowed to skip
  retrieval by default
- prompt refresh no longer depends on prompt length alone
- query-aware refresh falls back cleanly to general context retrieval when
  needed
- any non-empty auto-retrieved context is injected into the active turn
- file/search/write enrichment is capability-driven, not string-list-driven
- eligible pre-tool turns attempt enrichment by default when injection is
  enabled
- automatic injection remains `turn` scope by default
- token cost and exact-duplicate suppression policy are explicit and testable
- human and assistant recall differ only where the product intentionally says
  they differ
- every injected memory block has visible provenance and scope
- Codex acts as a strong caller of `agentmemory`'s retrieval model rather than
  reimplementing it

## Open Questions

- Do we eventually want a backend endpoint that returns ranked candidates plus
  token estimates, or is Codex-side planning sufficient?

## Related Docs

- [`agentmemory_runtime_surface_spec.md`](./agentmemory_runtime_surface_spec.md)
- [`agentmemory_runtime_expansion_followup_spec.md`](./agentmemory_runtime_expansion_followup_spec.md)
- [`../../docs/claude-code-hooks-parity.md`](../../docs/claude-code-hooks-parity.md)
