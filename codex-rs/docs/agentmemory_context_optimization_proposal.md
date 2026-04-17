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
- and whether the token cost was justified.

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
- and whether a previously injected result should be suppressed as a duplicate.

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
- cross-turn reinjection suppression for already injected context blocks

Two layers of dedupe are correct:

- backend dedupe:
  - deduplicate candidate memory blocks within one retrieval result
- Codex dedupe:
  - avoid reinjecting the same returned context block on adjacent turns with no
    new signal

### 4. Replace prompt-length refresh with signal-based refresh

Stop using prompt length as the main retrieval heuristic.

Refresh should trigger on signal such as:

- prompt references a file, module, bug, ticket, or earlier decision
- resumed thread or post-compaction turn
- cwd/project/branch changes
- the prompt materially changes from the last retrieval query
- previous tool output created a new retrieval opportunity

Refresh should skip when:

- the prompt is a trivial steer with no new retrieval signal
- the same reinjection key was injected recently
- the planner budget is exhausted

Rationale:

- retrieval value is semantic, not length-based

When refresh is selected:

- first call `/agentmemory/context/refresh` with the query
- if that returns `skipped = true`, empty context, or no context at all, and
  retrieval still appears warranted, fall back to `/agentmemory/context` with
  an explicit budget

That fallback already exists in another `agentmemory` integration and should be
the Codex behavior too.

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

Aggressive pre-tool behavior:

- structured file or term inputs should be forwarded whenever available
- no mode gate or secondary selectiveness gate beyond capability eligibility
- no shell / exec auto-enrichment
- no network-mutating auto-enrichment
- Codex may still suppress exact repeated reinjection of identical returned
  context within the same turn or on adjacent turns

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
- if assistant needs durable injection, it must request it explicitly via:
  - `memory_recall` with `scope: "thread"`, or
  - a second explicit tool if we prefer not to widen the current schema

No silent promotion from `turn` to `thread`.

### 7. Add caller budgets and reinjection suppression

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
- avoid reinjecting identical context on adjacent turns unless reason or query
  changed
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
- enabled injection can inject on meaningful user turns without the
  prompt-length heuristic
- enabled injection falls back from `/context/refresh` to `/context` when query
  refresh yields no context but retrieval is still warranted
- capability-based pre-tool enrichment covers tools with structured
  `agentmemory_input`, not just hardcoded names
- identical recalled context is not reinjected repeatedly without a new reason
  or stale-window expiry
- every eligible capability-class pre-tool turn attempts enrichment when
  injection is enabled
- human recall persists to thread history with explicit scope metadata
- assistant recall can remain turn-local or explicitly persist to thread
- replay/resume shows why a memory block was injected and under what scope
- caller budgets are passed to `agentmemory` deterministically
- Codex does not duplicate agentmemory-side lane ranking or intra-response
  deduplication logic

## Acceptance Criteria

This proposal is implemented well when all of the following are true:

- users who run with `agentmemory` enabled get more relevant automatic recall
  than the current parity lane
- prompt refresh no longer depends on prompt length alone
- query-aware refresh falls back cleanly to general context retrieval when
  needed
- file/search/write enrichment is capability-driven, not string-list-driven
- eligible pre-tool turns attempt enrichment by default when injection is
  enabled
- token cost and reinjection policy are explicit and testable
- human and assistant recall differ only where the product intentionally says
  they differ
- every injected memory block has visible provenance and scope
- Codex acts as a strong caller of `agentmemory`'s retrieval model rather than
  reimplementing it

## Open Questions

- Is assistant thread injection better modeled as a `memory_recall` scope enum
  or as a second explicit tool?
- Do we eventually want a backend endpoint that returns ranked candidates plus
  token estimates, or is Codex-side planning sufficient?

## Related Docs

- [`agentmemory_runtime_surface_spec.md`](./agentmemory_runtime_surface_spec.md)
- [`agentmemory_runtime_expansion_followup_spec.md`](./agentmemory_runtime_expansion_followup_spec.md)
- [`../../docs/claude-code-hooks-parity.md`](../../docs/claude-code-hooks-parity.md)
