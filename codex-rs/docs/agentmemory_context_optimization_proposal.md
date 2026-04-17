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

## Current Problems

### 1. One boolean controls multiple strategies

`inject_context = true` currently means "turn on the parity lane", but it does
not say how aggressive or conservative retrieval should be. The operator cannot
express:

- disable auto injection entirely
- keep the current parity behavior
- use a more retrieval-forward balanced mode
- use a more aggressive mode with larger budgets

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

Codex should add a unified `agentmemory` context planner and treat the current
boolean injection flag as a compatibility shim, not the long-term product
surface.

The planner should decide:

- whether retrieval is warranted,
- which backend endpoint to call,
- how much context budget is available,
- whether the result should be turn-local or thread-persisted,
- and whether a candidate should be suppressed as a duplicate.

This keeps the current backend contract intact while making behavior coherent.

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

### 1. Introduce `context_policy`

Replace the one-bit model with an explicit policy enum:

- `off`
- `parity`
- `balanced`
- `aggressive`

Compatibility rules:

- `inject_context = false` maps to `off`
- `inject_context = true` with no explicit policy maps to `parity`
- docs should recommend `balanced` for operators who intentionally run
  `agentmemory` as a normal always-on part of their workflow

Rationale:

- preserves backward compatibility
- separates "enabled" from "how hard should we optimize"
- gives the product a place to evolve without overloading one boolean

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
- `budget_tokens`
- `inject_scope`
  - `none`
  - `turn`
  - `thread`
- `dedupe_key`
- `skip_reason`, when suppressed

Design rule:

- backend endpoints are transport details
- product semantics live in the planner

### 3. Replace prompt-length refresh with signal-based refresh

Stop using prompt length as the main retrieval heuristic.

Refresh should trigger on signal such as:

- prompt references a file, module, bug, ticket, or earlier decision
- resumed thread or post-compaction turn
- cwd/project/branch changes
- the prompt materially changes from the last retrieval query
- previous tool output created a new retrieval opportunity

Refresh should skip when:

- the prompt is a trivial steer with no new retrieval signal
- the same dedupe key was injected recently
- the planner budget is exhausted

Rationale:

- retrieval value is semantic, not length-based

### 4. Classify tool enrichment by capability

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

### 5. Make persistence explicit

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

### 6. Add budgets and dedupe

Introduce explicit caps such as:

- `startup_context_budget_tokens`
- `turn_context_budget_tokens`
- `pretool_context_budget_tokens`
- `max_auto_injections_per_turn`
- `reinject_after_turns`

Planner requirements:

- hash normalized context before injection
- avoid reinjecting identical context on adjacent turns unless reason or query
  changed
- down-rank stale context that was already seen recently
- record token estimates for each injected block

### 7. Make injection visible

Every inject or skip decision should emit structured state:

- source endpoint
- reason
- query
- estimated token count
- dedupe outcome
- scope
- whether content was actually injected

Human-visible surfaces should make it obvious:

- that `agentmemory` injected context for the turn
- why it did so
- whether it persisted to thread history
- whether it skipped due to dedupe or budget

### 8. Keep the initial backend contract stable

The first implementation phase should keep using the existing endpoints:

- `POST /agentmemory/session/start`
- `POST /agentmemory/context/refresh`
- `POST /agentmemory/enrich`
- `POST /agentmemory/context`

This proposal does not require a new backend planning endpoint up front.

## Configuration Proposal

Add a richer config surface:

```toml
[memories.agentmemory]
context_policy = "parity" # off | parity | balanced | aggressive
startup_context_budget_tokens = 300
turn_context_budget_tokens = 600
pretool_context_budget_tokens = 300
max_auto_injections_per_turn = 2
reinject_after_turns = 8
assistant_recall_thread_injection = "explicit" # explicit | disabled
```

Compatibility rules:

- existing `inject_context` remains supported
- config loading maps old boolean semantics when `context_policy` is absent
- environment overrides remain supported for compatibility, but docs should
  prefer the policy-based config surface

## Behavior By Policy

### `off`

- no startup injection
- no prompt-refresh injection
- no pre-tool enrichment
- explicit human/assistant recall still works when invoked directly

### `parity`

- preserve current Claude-parity behavior
- keep conservative tool coverage
- useful as the compatibility/default mapping for old boolean setups

### `balanced`

- recommended for operators who intentionally run `agentmemory` as part of
  normal workflow
- signal-based refresh on meaningful user turns
- capability-based pre-tool enrichment
- strict token and dedupe budgets
- explicit visibility for every inject/skip decision

### `aggressive`

- same planner model, looser budgets
- more willingness to refresh after large context shifts
- still no shell/exec auto-enrichment

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

- `context_policy` config
- planner output type
- budget/dedupe tracking
- no behavior expansion beyond `parity`

### Phase 2. Replace brittle heuristics

Primary files:

- `core/src/hook_runtime.rs`
- `core/src/tools/registry.rs`
- tool handlers that already populate `agentmemory_input`

Deliverables:

- signal-based prompt refresh
- capability-based pre-tool eligibility
- tests for dedupe and skip reasons

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
- recommendation to use `balanced` when `agentmemory` is intentionally
  always-on
- full acceptance-test coverage

## Verification

Add or update tests that prove:

- `parity` mode preserves current behavior
- `balanced` mode can inject on meaningful user turns without the prompt-length
  heuristic
- capability-based pre-tool enrichment covers tools with structured
  `agentmemory_input`, not just hardcoded names
- identical recalled context is not reinjected repeatedly without a new reason
  or stale-window expiry
- human recall persists to thread history with explicit scope metadata
- assistant recall can remain turn-local or explicitly persist to thread
- replay/resume shows why a memory block was injected and under what scope
- token budgets cap injected context deterministically

## Acceptance Criteria

This proposal is implemented well when all of the following are true:

- users who already run with `agentmemory` enabled get more relevant automatic
  recall than the current parity lane
- prompt refresh no longer depends on prompt length alone
- file/search/write enrichment is capability-driven, not string-list-driven
- token cost and dedupe policy are explicit and testable
- human and assistant recall differ only where the product intentionally says
  they differ
- every injected memory block has visible provenance and scope
- parity users can stay conservative without opt-in breakage

## Open Questions

- Should `balanced` become the documented recommendation as soon as the
  planner lands, or only after token-cost validation?
- Is assistant thread injection better modeled as a `memory_recall` scope enum
  or as a second explicit tool?
- Do we eventually want a backend endpoint that returns ranked candidates plus
  token estimates, or is Codex-side planning sufficient?

## Related Docs

- [`agentmemory_runtime_surface_spec.md`](./agentmemory_runtime_surface_spec.md)
- [`agentmemory_runtime_expansion_followup_spec.md`](./agentmemory_runtime_expansion_followup_spec.md)
- [`../../docs/claude-code-hooks-parity.md`](../../docs/claude-code-hooks-parity.md)
