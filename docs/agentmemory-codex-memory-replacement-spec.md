# agentmemory replacement spec for Codex native memory

This document evaluates whether a forked Codex should disable the current
first-party memory system and replace it with
`~/Projects/agentmemory` as the primary memory engine.

It is intended to be the canonical decision and implementation handoff for this
specific question:

- is `agentmemory` materially more capable than Codex native memory,
- is it likely more token-efficient over time,
- if so, what would be lost by replacing Codex native memory,
- what replacement shape is worth building in a fork.

This is an architecture and product-integration spec, not a request to
implement the replacement immediately.

## Executive summary

`agentmemory` is materially more advanced than Codex native memory as a
capture and retrieval engine.

The strongest deltas are:

- broader lifecycle capture through a wider hook surface,
- hybrid retrieval (BM25 + vector + graph),
- pluggable embeddings including Gemini,
- cross-agent MCP and REST exposure,
- retrieval bounded by top-K / token-budgeted context instead of relying on a
  prebuilt local memory summary alone.

Codex native memory is still stronger in first-party runtime integration:

- startup memory generation is built into the core session lifecycle,
- memory artifacts are deeply integrated into prompt construction,
- memory citations already flow through protocol and app-server surfaces,
- there are explicit local operations for memory refresh and memory removal,
- native memory state includes thread-level `memory_mode` semantics such as
  `disabled` and `polluted`.

Conclusion:

- `agentmemory` is a material capability superset for memory retrieval and
  capture quality.
- It is not a strict end-to-end product superset of Codex native memory.
- A replacement is defensible, but only if the fork rebuilds a thin Codex
  integration layer for the native semantics that matter.

Recommended direction:

- do not pursue "full Claude parity first",
- do pursue "agentmemory as the primary memory engine with Codex-specific
  shims",
- disable Codex native memory generation only after startup injection,
  replacement memory ops, and a clear citation story are decided.

## Target end state

The target end state is not "agentmemory instead of Codex" in a narrow sense.
The target end state is:

- \`agentmemory\` is the primary memory engine,
- Codex-native memory generation and consolidation are disabled,
- Codex retains or rebuilds only the product-level semantics that still add
  value,
- the fork uses the full \`agentmemory\` retrieval stack in steady state:
  BM25 + vector + graph,
- embeddings are enabled by default in steady state; BM25-only mode is a
  fallback, not the target architecture,
- lifecycle capture uses the widest useful hook surface rather than the minimum
  viable subset,
- the fork presents one coherent memory system to users,
- the resulting system is a functional superset of both:
  - \`agentmemory\` capture and retrieval strengths,
  - Codex-native prompt/runtime/protocol integration where it materially helps.

In other words, the desired architecture is a Venn-diagram merge with one
authoritative engine, not permanent coexistence of two competing memory stacks.

## Maximum-performance policy

The intended end state should maximize \`agentmemory\`, not merely adopt it.

That means:

- use hybrid retrieval as the primary retrieval path,
- enable embeddings by default in the intended production configuration,
- preserve graph retrieval and relation-aware retrieval as first-class
  capabilities,
- use progressive disclosure and token budgets instead of large static memory
  injections wherever possible,
- implement enough hook coverage that the observation stream is rich rather
  than sparse,
- treat BM25-only mode as an acceptable degraded mode, not as the target.

Provider policy:

- support all current \`agentmemory\` embedding providers,
- keep Gemini embeddings available as a first-class provider,
- prefer the best available embedding backend for the environment rather than
  hardcoding a low-capability default in the architecture,
- avoid designing the replacement around a no-embeddings baseline.

## Scope

This spec compares:

- `/private/tmp/codex`
- `/Users/ericjuta/Projects/agentmemory`

This spec is based on the current implementation shape in those checkouts,
including user-local plugin and hook configuration files present in the
`agentmemory` repo.

## Read order

Read these sources in order if implementing against this spec:

1. `docs/agentmemory-codex-memory-replacement-spec.md`
2. `docs/claude-code-hooks-parity.md`
3. `codex-rs/core/src/memories/README.md`
4. `codex-rs/core/src/memories/prompts.rs`
5. `codex-rs/core/templates/memories/read_path.md`
6. `codex-rs/core/src/codex.rs`
7. `codex-rs/hooks/src/engine/config.rs`
8. `codex-rs/hooks/src/engine/discovery.rs`
9. `plugin/hooks/hooks.json` in `agentmemory`
10. `src/hooks/*.ts` in `agentmemory`
11. `src/providers/embedding/*.ts` and `src/state/hybrid-search.ts` in
    `agentmemory`
12. `README.md` and `benchmark/*.md` in `agentmemory`

## Current source snapshot

### Codex

Codex currently has:

- a first-party startup memory pipeline in
  `codex-rs/core/src/memories/README.md`,
- phase-1 extraction and phase-2 consolidation into `MEMORY.md`,
  `memory_summary.md`, and rollout summary artifacts,
- developer-prompt injection of memory read-path instructions built from
  `memory_summary.md`,
- protocol-level memory citations,
- memory-management operations such as `UpdateMemories` and
  `DropMemories`,
- thread-level memory-mode state such as `disabled` and `polluted`,
- an under-development `codex_hooks` feature with five public hook events.

### agentmemory

The `agentmemory` checkout currently contains:

- a plugin manifest in `plugin/plugin.json`,
- a Claude-oriented hook bundle in `plugin/hooks/hooks.json`,
- TypeScript hook entrypoints under `src/hooks/`,
- multiple embedding providers under `src/providers/embedding/`,
- hybrid retrieval under `src/state/hybrid-search.ts`,
- REST and MCP surfaces,
- benchmarking and retrieval claims in `README.md` and `benchmark/`.

The local `agentmemory` checkout is currently dirty. This matters only as a
reminder not to treat the local repo state as release-tagged truth; the source
shape is still adequate for architectural comparison.

### Current env alignment

The live worker configuration is not sourced from
`~/Projects/agentmemory/.env`. In this checkout, `docker-compose.yml` points
the worker at:

- `\${HOME}/.agentmemory/.env`

Current externally loaded env alignment, verified in redacted form:

- `GEMINI_API_KEY` is present,
- `GEMINI_MODEL` is present,
- `GEMINI_EMBEDDING_MODEL` is present,
- `GEMINI_EMBEDDING_DIMENSIONS` is present,
- `GRAPH_EXTRACTION_ENABLED` is present,
- `CONSOLIDATION_ENABLED` is present.

Implications:

- the current live environment already aligns with Gemini-first provider
  selection,
- embedding auto-detection should resolve to Gemini unless explicitly
  overridden,
- graph extraction and consolidation are already enabled in the current
  external env,
- the current external env does not explicitly pin `EMBEDDING_PROVIDER`,
  `TOKEN_BUDGET`, `BM25_WEIGHT`, `VECTOR_WEIGHT`, or
  `FALLBACK_PROVIDERS`, so those currently rely on code defaults rather than
  explicit ops policy.

For a maximum-performance steady state, that last point should be treated as a
configuration gap, not as the desired final setup.

## Codex native memory: what it is

Codex native memory is a core-managed memory pipeline, not just a retrieval
plugin.

### Pipeline shape

Codex native memory runs in two phases:

1. Phase 1 extracts structured memory from eligible rollouts and stores
   stage-1 outputs in the state DB.
2. Phase 2 consolidates those stage-1 outputs into durable memory artifacts on
   disk and spawns an internal consolidation subagent.

This is documented in `codex-rs/core/src/memories/README.md`.

### Prompt integration

Codex adds memory usage instructions directly into developer instructions when:

- the memory feature is enabled,
- `config.memories.use_memories` is true,
- memory summary content exists.

This is wired in `codex-rs/core/src/codex.rs` via
`build_memory_tool_developer_instructions(...)`.

### Artifact model

Codex memory produces and maintains:

- `memory_summary.md`
- `MEMORY.md`
- `raw_memories.md`
- `rollout_summaries/*`
- optional `skills/*`

These artifacts are not just storage. They are part of how Codex routes future
memory reads and citations.

### Operational integration

Codex exposes native memory operations:

- `UpdateMemories`
- `DropMemories`

and memory-state controls:

- `generate_memories`
- `use_memories`
- `no_memories_if_mcp_or_web_search`

Codex also tracks thread memory-mode transitions such as `polluted`.

### Citation integration

Codex has protocol and app-server support for structured memory citations.
Those citations are already part of assistant-message rendering and transport.

## agentmemory: what it is

`agentmemory` is not just a memory file or summary generator. It is a
capture, indexing, retrieval, consolidation, MCP, and REST system.

### Capture model

The working Claude-oriented setup uses 12 hooks:

- `SessionStart`
- `UserPromptSubmit`
- `PreToolUse`
- `PostToolUse`
- `PostToolUseFailure`
- `PreCompact`
- `SubagentStart`
- `SubagentStop`
- `Notification`
- `TaskCompleted`
- `Stop`
- `SessionEnd`

The hook bundle is defined in `plugin/hooks/hooks.json`.

### Observation flow

The core runtime pattern is:

- hooks send observations to REST endpoints,
- observations are deduplicated and privacy-filtered,
- observations are compressed and indexed,
- retrieval returns bounded context back into future sessions.

The important thing is that capture happens at many lifecycle points, not just
after a Codex-style rollout completes.

### Retrieval model

agentmemory uses:

- BM25,
- vector retrieval,
- graph retrieval,
- Reciprocal Rank Fusion,
- session diversification,
- progressive disclosure.

This is a genuine retrieval stack, not just a durable handbook.

### Embeddings

agentmemory supports multiple embedding providers, including:

- local embeddings,
- Gemini embeddings,
- OpenAI embeddings,
- Voyage,
- Cohere,
- OpenRouter.

Gemini embedding support is real in this checkout, not hypothetical.

### Cross-agent model

agentmemory is designed as a shared external service:

- Claude hooks can write to it,
- MCP clients can query it,
- REST clients can integrate with it,
- multiple agent products can share one instance.

This is a major design difference from Codex native memory.

## Capability comparison

### Capture breadth

Codex native memory:

- captures memory from rollouts selected by startup pipeline rules,
- is optimized around per-rollout extraction and later consolidation,
- does not expose comparable public lifecycle capture breadth in the current
  hook surface.

agentmemory:

- captures at many lifecycle points,
- can record prompts, tool usage, failures, compaction moments, and subagent
  lifecycle events,
- better matches the event stream of real coding work.

Verdict:

- `agentmemory` is materially stronger.

### Retrieval quality

Codex native memory:

- primarily relies on generated memory artifacts,
- injects a read-path and memory summary into the prompt,
- does not show comparable semantic retrieval, vector search, BM25 ranking, or
  graph traversal in the native memory path from the current source scan.

agentmemory:

- provides hybrid search,
- supports embeddings,
- supports graph-aware retrieval,
- uses token-bounded context assembly.

Verdict:

- `agentmemory` is materially stronger.

### Consolidation sophistication

Codex native memory:

- has a robust two-phase extraction and consolidation pipeline,
- uses a dedicated consolidation subagent,
- maintains curated memory artifacts intended for future prompt routing.

agentmemory:

- claims 4-tier consolidation and memory evolution,
- versioning, semantic/procedural layers, relation graphs, and cascading
  staleness.

Verdict:

- `agentmemory` is likely more ambitious and broader,
- Codex native memory is more tightly integrated and operationally simpler
  inside Codex.

### First-party runtime integration

Codex native memory:

- is already first-party,
- already has prompt integration,
- already has memory commands,
- already has citations,
- already participates in internal policy/state flows.

agentmemory:

- does not automatically provide those Codex-native product behaviors,
- would need a Codex-specific bridge layer to replace them cleanly.

Verdict:

- Codex native memory is stronger here.

### Cross-agent reuse

Codex native memory:

- is local to Codex runtime and artifacts.

agentmemory:

- is designed for multi-agent reuse through MCP and REST.

Verdict:

- `agentmemory` is materially stronger.

## Is agentmemory a material superset?

### Yes, in these senses

agentmemory is a material superset of Codex native memory for:

- retrieval breadth,
- semantic search,
- embedding-backed lookup,
- graph-backed lookup,
- cross-agent sharing,
- hook-based observation capture.

### No, in these senses

agentmemory is not a strict product-level superset of Codex native memory for:

- first-party startup prompt integration,
- native memory operations (`UpdateMemories`, `DropMemories`),
- native memory citation protocol plumbing,
- thread-level memory-mode semantics such as `polluted`,
- deep alignment with Codex’s state DB and app-server/TUI surfaces.

The correct judgment is:

- `agentmemory` is a material capability superset for retrieval and capture,
- not a strict end-to-end replacement unless shims are added.

The desired fork outcome therefore is:

- replace Codex native memory internals,
- preserve or rebuild the useful Codex-native user-facing semantics as shims,
- end with a product-level superset even though `agentmemory` alone is not a
  strict superset today.

## Token efficiency

This is the strongest practical argument in favor of `agentmemory`.

### Strong evidence in favor of agentmemory

The `agentmemory` repo explicitly claims and benchmarks token savings:

- `~1,900` tokens instead of loading all memory into context in
  `README.md`,
- `92%` savings in `benchmark/REAL-EMBEDDINGS.md`,
- `86%` savings in `benchmark/QUALITY.md`,
- essentially corpus-size-stable top-K retrieval in `benchmark/SCALE.md`.

The architectural reason is coherent:

- retrieval returns top-K results,
- context assembly is bounded,
- compact result-first progressive disclosure reduces unnecessary expansion.

### Codex native memory token profile

Codex native memory is not obviously awful on tokens, but it is shaped
differently:

- `memory_summary.md` injection is truncated to `5,000` tokens in
  `codex-rs/core/src/memories/mod.rs`,
- stage-1 rollout processing can consume large inputs because it is an offline
  extraction pipeline, not a lightweight query-time retrieval layer,
- the memory read-path instructs the model to query local memory artifacts
  rather than receiving a purpose-built top-K retrieval result from a hybrid
  search engine.

### Apples-to-oranges caution

The token comparison is not perfectly head-to-head.

agentmemory benchmarks compare against "load everything into context" and
built-in-memory patterns such as monolithic `CLAUDE.md`-style memory files.
Codex native memory is more curated than that:

- it injects a bounded `memory_summary.md`,
- it exposes a read-path for progressive on-disk lookup,
- it does not appear to simply dump all historical memory into every turn.

So it would be wrong to claim the benchmark proves "agentmemory is 92% more
token-efficient than Codex native memory" as a verified current fact.

### Bottom-line token judgment

Even with that caveat, `agentmemory` is still likely more token-efficient over
the long term than Codex native memory for large corpora because:

- query-time retrieval is explicitly bounded,
- corpus growth does not force proportional prompt growth,
- embedding + hybrid retrieval reduces the need to over-inject summaries "just
  in case",
- progressive disclosure lets the system fetch more only when needed.

Codex native memory likely remains acceptable for modest corpus sizes, but it
does not appear to have the same query-time retrieval efficiency model.

## Replacement architecture

### Option 1: hard replacement

Disable Codex native memory generation and injection entirely. Make
`agentmemory` the only memory engine.

Benefits:

- cleaner mental model,
- no duplicate memory systems,
- retrieval quality and token efficiency become `agentmemory`-driven,
- cross-agent memory reuse becomes first-class.

Costs:

- must rebuild startup prompt integration,
- must replace or remove `UpdateMemories` and `DropMemories`,
- must decide what to do about native memory citations,
- must replace or drop `polluted`/thread memory-mode semantics,
- must extend Codex hooks enough to make capture quality fully competitive with
  the `agentmemory` model rather than merely acceptable.

Risk:

- highest.

## Native Codex behaviors that replacement must preserve or intentionally drop

### Must preserve or replace

- startup injection into developer instructions,
- user-facing operations to refresh or clear memory state,
- some citation strategy if memory provenance is important,
- protocol/app-server awareness of whatever replaces native memory,
- a clear policy for memory invalidation / pollution.

### Safe to drop if explicitly accepted

- on-disk `MEMORY.md` / `memory_summary.md` artifact format compatibility,
- the exact current phase-1 / phase-2 internal implementation,
- native Codex consolidation subagent if `agentmemory` becomes authoritative,
- native artifact grooming and rollout summary persistence if the fork no longer
  treats those as the canonical memory store.

## Key risks

### Duplicate system ambiguity

If both systems remain partially active, it becomes unclear:

- which system is authoritative,
- which one should inject context,
- which one should be cited,
- which one should be cleared by a user-facing "drop memories" action.

Avoid this.

### Hook-surface insufficiency

Current Codex hooks are not enough to reproduce Claude-style `agentmemory`
capture quality:

- only five public events,
- sync command handlers only,
- narrower tool coverage,
- missing public equivalents for several useful lifecycle events.

If the fork does not extend hooks, the replacement will still leave value on
the table.

### Protocol and UX regressions

Dropping native Codex memory without replacing its protocol-level behaviors can
regress:

- assistant memory citations,
- memory-management commands,
- app-server/TUI expectations around memory-aware behavior.

### Benchmark over-claiming

Do not claim:

- that the `agentmemory` benchmarks directly prove a specific percentage gain
  over Codex native memory,
- or that Gemini embeddings alone guarantee better results.

The right claim is narrower:

- `agentmemory` has a more scalable retrieval architecture and published token
  savings versus all-in-context memory loading approaches,
- and that architecture is likely better long-term than Codex native memory for
  large memory corpora.

### Performance-oriented token policy

The intended architecture should optimize for query-time token efficiency, not
artifact compatibility.

That means:

- prefer top-K retrieval over broad handbook injection,
- keep startup context bounded and relevance-ranked,
- expand details only on demand,
- avoid recreating a large static `MEMORY.md`-style injection layer on top of
  `agentmemory`,
- measure steady-state tokens/query as a first-class success metric.

## Recommendation

Target hard replacement as the end state.

That means:

1. make `agentmemory` the sole authoritative memory engine,
2. disable Codex native memory generation and consolidation in the final
   architecture,
3. rebuild only the Codex-native product semantics worth preserving as shims on
   top of `agentmemory`,
4. remove or deprecate native Codex memory artifacts and workflows in the fork
   once those shims exist.

This is the recommended path because it matches the explicit desired outcome:

- one memory authority,
- no split-brain behavior,
- `agentmemory` for the stronger retrieval and capture substrate,
- Codex integration retained only where it improves the product.

The fork can still phase the work, but every phase should point toward hard
replacement rather than toward permanent coexistence.

## Recommended implementation phases

### Phase 1: decision and contract

- Decide that `agentmemory` is the primary memory authority.
- Freeze which native Codex behaviors will be preserved.
- Define how startup context injection will work in the fork.
- Decide whether native memory citations remain required.
- Define the end-state explicitly as a functional superset, not a partial port.

### Phase 2: Codex integration adapter

- Add a Codex-specific `agentmemory` integration layer.
- Replace startup memory prompt generation with `agentmemory` retrieval.
- Add equivalent user-facing operations for refresh and clear.
- Decide whether these call into `agentmemory` REST/MCP or a local adapter.
- Route startup injection through the bounded `agentmemory` retrieval path
  rather than recreating Codex-native memory artifact loading.
- Make token budget, retrieval mode, and expansion behavior explicit parts of
  the adapter contract.

### Phase 3: hook expansion

- Extend Codex hook coverage to support the full useful `agentmemory`
  observation model, not just a minimum subset.
- Target the full current `agentmemory` hook set:
  - `SessionStart`
  - `UserPromptSubmit`
  - `PreToolUse`
  - `PostToolUse`
  - `PostToolUseFailure`
  - `PreCompact`
  - `SubagentStart`
  - `SubagentStop`
  - `Notification`
  - `TaskCompleted`
  - `Stop`
  - `SessionEnd`
- Broaden `PreToolUse` and `PostToolUse` beyond the current shell-centric
  path so file tools, command tools, and other high-signal tool classes are
  observed consistently.
- Do not treat hook expansion as optional polish; it is core to achieving the
  high-performance end state.

### Phase 4: native memory deprecation

- Turn off Codex native memory generation by default in the fork.
- Remove or quarantine old native memory artifacts once the adapter is stable.
- Preserve migration tooling only if existing users need it.

### Phase 5: superset hardening

- Verify that every retained Codex-native memory affordance has an
  `agentmemory`-backed implementation or an intentional deletion note.
- Verify that token usage remains bounded as corpus size grows.
- Verify that there is only one authoritative memory source in the runtime.
- Remove any remaining code paths that can accidentally re-enable split-brain
  behavior.
- Verify that embeddings, graph retrieval, and progressive disclosure are
  active in the intended steady-state configuration.
- Verify that the system is not silently falling back to a lower-capability
  retrieval mode in normal operation.

### Phase 6: optional advanced alignment

- Add memory citation mapping from `agentmemory` results into Codex protocol
  structures.
- Add richer protocol and app-server visibility if needed.
- Reassess whether any remaining native memory logic should survive.

## Execution plan

This section turns the replacement architecture into a low-rebase execution
plan.

The key rule is:

- keep invasive edits concentrated in a few upstream-hot orchestration files,
- keep most new logic in fork-owned modules,
- gate native behavior off before deleting it.

### Allowed write boundaries

The preferred fork seam is:

- small edits in:
  - `codex-rs/core/src/codex.rs`
  - `codex-rs/core/src/hook_runtime.rs`
  - `codex-rs/hooks/src/engine/config.rs`
  - `codex-rs/hooks/src/engine/discovery.rs`
  - hook event/schema files only when required for new public events
- most new implementation in new fork-owned modules, for example:
  - `codex-rs/core/src/agentmemory/`
  - `codex-rs/hooks/src/agentmemory/` or equivalent hook-translation module

### Intentionally untouched until cutover

Do not broadly rewrite these early:

- `codex-rs/core/src/memories/*`
- `codex-rs/core/templates/memories/*`
- native memory artifact generation logic
- broad protocol/app-server surfaces unrelated to memory provenance

Early phases should gate or bypass these paths, not delete or refactor them.

### Branch order

Use a short stack of focused branches / PRs.

#### PR 1: backend selector and fork seam

Goal:

- introduce a clear memory backend selector,
- add the new `agentmemory` adapter module skeleton,
- make no user-visible behavior change yet.

Write scope:

- config wiring,
- new adapter modules,
- minimal callsite plumbing only where needed.

Must not do:

- no native memory deletion,
- no protocol changes,
- no hook expansion yet.

Merge gate:

- no behavior regression with native memory still active by default,
- docs updated to describe the seam.

#### PR 2: startup injection replacement

Goal:

- route startup memory injection through the `agentmemory` adapter,
- make bounded retrieval the new startup path,
- stop depending on native memory artifact loading for startup context.

Write scope:

- `codex-rs/core/src/codex.rs`
- adapter module
- minimal config/docs updates

Must not do:

- do not delete native memories yet,
- do not add broad protocol changes,
- do not expand hook coverage in the same PR.

Merge gate:

- startup context is sourced from `agentmemory`,
- token budget and retrieval mode are explicit and tested,
- no static `MEMORY.md`-style reinjection layer is recreated on top.

#### PR 3: public hook event expansion

Goal:

- expand Codex hooks to cover the full useful `agentmemory` hook set:
  - `SessionStart`
  - `UserPromptSubmit`
  - `PreToolUse`
  - `PostToolUse`
  - `PostToolUseFailure`
  - `PreCompact`
  - `SubagentStart`
  - `SubagentStop`
  - `Notification`
  - `TaskCompleted`
  - `Stop`
  - `SessionEnd`

Write scope:

- hook config/schema/discovery/runtime files,
- TUI/app-server visibility only where hook runs need surfacing.

Must not do:

- do not mix in native memory deletion,
- do not mix in citation replacement.

Merge gate:

- each event has runtime dispatch,
- each event is documented,
- hook run visibility remains coherent.

#### PR 4: tool coverage broadening

Goal:

- broaden `PreToolUse` and `PostToolUse` beyond the shell-centric path,
- ensure file tools, command tools, and other high-signal tool classes are
  observed consistently for `agentmemory`.

Write scope:

- `codex-rs/core/src/hook_runtime.rs`
- tool handler payload plumbing
- hook translation layer

Must not do:

- do not mix in memory command replacement,
- do not delete native memory paths yet.

Merge gate:

- high-signal tool classes emit useful observation payloads,
- no regression in existing shell-hook flows.

#### PR 5: memory ops and provenance replacement

Goal:

- replace or redefine `UpdateMemories` and `DropMemories`,
- decide and implement provenance behavior,
- define the replacement for native `polluted` semantics.

Write scope:

- memory command handlers,
- provenance/citation integration,
- minimal protocol additions if absolutely required.

Must not do:

- do not combine this with broad deletion of native memory code.

Merge gate:

- user-facing memory refresh/clear actions still exist or are intentionally
  documented as removed,
- provenance behavior is explicit,
- no ambiguity remains about memory invalidation rules.

#### PR 6: hard cutover

Goal:

- disable native memory generation and consolidation in normal runtime paths,
- make `agentmemory` the only authoritative memory backend,
- quarantine or deprecate native memory artifacts.

Write scope:

- backend selection defaults,
- final cutover gating,
- cleanup of callsites that can still route to native memory.

Must not do:

- do not do broad code deletion unless the fork is already stable after cutover,
- do not remove debug/rollback switches until at least one successful rebase
  cycle after cutover.

Merge gate:

- one memory authority in runtime,
- no split-brain injection,
- no accidental native fallback in standard flows.

#### PR 7: post-cutover cleanup

Goal:

- remove dead native-memory paths only after the hard cutover has stabilized.

Write scope:

- native memory code and docs that are no longer reachable,
- migration notes if existing users need them.

Merge gate:

- cleanup produces less rebase churn than it creates,
- rollback path is no longer needed.

### Rebase policy

- Rebase frequently; do not let this stack drift for long.
- Rebase before opening each PR and after any upstream changes touching:
  - `codex-rs/core/src/codex.rs`
  - `codex-rs/core/src/hook_runtime.rs`
  - hook engine config/discovery/schema/runtime files
- Prefer new modules over editing existing modules repeatedly.
- If a behavior can live in the adapter, keep it out of upstream-hot files.
- Do not delete upstream code early; disabling is cheaper to rebase than
  removal.

### Success metrics by PR

- PR 1: seam exists with no behavior regression.
- PR 2: startup injection is `agentmemory`-backed and token-bounded.
- PR 3: hook surface matches the intended `agentmemory` event model.
- PR 4: observation capture is rich across the important tool classes.
- PR 5: memory ops and provenance no longer depend on native memory internals.
- PR 6: runtime has one authoritative memory backend.
- PR 7: dead code removal does not increase future rebase cost materially.

### Handoff prompts by PR

These are intended as copy-paste prompts for future sessions, child agents, or
parallel worker swarms. Each prompt is deliberately scoped to one PR-sized
slice.

#### PR 1 handoff prompt

```text
Implement PR 1 from docs/agentmemory-codex-memory-replacement-spec.md.

Goal:
- introduce a clear memory backend selector
- add the new agentmemory adapter seam
- make no user-visible behavior change yet

Constraints:
- keep invasive edits concentrated
- do not delete or broadly rewrite codex-rs/core/src/memories/*
- do not change protocol shapes
- do not expand hooks yet

Write scope:
- config wiring
- new fork-owned adapter modules
- minimal callsite plumbing only where needed

Acceptance:
- native memory remains default and behaviorally unchanged
- the seam exists and is documented
- code is structured so later PRs can route through the adapter without large rewrites
```

#### PR 2 handoff prompt

```text
Implement PR 2 from docs/agentmemory-codex-memory-replacement-spec.md.

Goal:
- replace startup memory prompt generation with agentmemory-backed retrieval
- make startup context bounded, relevance-ranked, and token-budgeted

Constraints:
- do not recreate static MEMORY.md-style loading on top of agentmemory
- do not expand hooks in this PR
- do not delete native memory code yet

Write scope:
- codex-rs/core/src/codex.rs
- agentmemory adapter module
- small config/docs updates if required

Acceptance:
- startup injection is sourced through the adapter
- retrieval mode and token budget are explicit
- native memory still exists only as a gated fallback path, not the main path
```

#### PR 3 handoff prompt

```text
Implement PR 3 from docs/agentmemory-codex-memory-replacement-spec.md.

Goal:
- expand Codex public hooks to support the full useful agentmemory event model

Target events:
- SessionStart
- UserPromptSubmit
- PreToolUse
- PostToolUse
- PostToolUseFailure
- PreCompact
- SubagentStart
- SubagentStop
- Notification
- TaskCompleted
- Stop
- SessionEnd

Constraints:
- keep handler semantics coherent
- do not mix in native memory deletion
- do not mix in provenance/citation replacement

Acceptance:
- each target event is represented in config/discovery/runtime
- documentation and hook-run visibility are updated
- new events do not regress existing hook behavior
```

#### PR 4 handoff prompt

```text
Implement PR 4 from docs/agentmemory-codex-memory-replacement-spec.md.

Goal:
- broaden PreToolUse and PostToolUse beyond the shell-centric path
- ensure high-signal tool classes produce useful agentmemory observations

Constraints:
- prioritize file tools, command tools, and other high-signal tool classes
- do not mix in memory command replacement
- do not cut over the backend here

Acceptance:
- important tool classes emit observation payloads consistently
- shell-hook behavior still works
- capture quality is materially closer to the Claude-side agentmemory model
```

#### PR 5 handoff prompt

```text
Implement PR 5 from docs/agentmemory-codex-memory-replacement-spec.md.

Goal:
- replace or redefine UpdateMemories and DropMemories
- decide and implement provenance behavior
- define the replacement for native polluted semantics

Constraints:
- keep protocol churn minimal unless required
- make user-facing behavior explicit
- do not delete native memory paths in this PR

Acceptance:
- memory refresh/clear actions still exist or are intentionally removed with docs
- provenance behavior is explicit
- invalidation rules are no longer ambiguous
```

#### PR 6 handoff prompt

```text
Implement PR 6 from docs/agentmemory-codex-memory-replacement-spec.md.

Goal:
- make agentmemory the only authoritative runtime memory backend
- disable native memory generation and consolidation in normal runtime paths

Constraints:
- do not do broad dead-code deletion yet
- keep rollback/debug switches until cutover is validated

Acceptance:
- one memory authority remains in runtime
- no split-brain injection is possible in standard flows
- native paths are gated off rather than accidentally still active
```

#### PR 7 handoff prompt

```text
Implement PR 7 from docs/agentmemory-codex-memory-replacement-spec.md.

Goal:
- perform post-cutover cleanup only after the hard replacement is stable

Constraints:
- prefer cleanup that reduces future rebase cost
- do not remove rollback/debug tooling prematurely

Acceptance:
- dead native-memory paths are removed only when safe
- cleanup does not create more rebase drag than it removes
```

#### Cross-PR reviewer prompt

```text
Review the current PR against docs/agentmemory-codex-memory-replacement-spec.md.

Focus:
- does this PR stay within its assigned write boundary
- does it reduce or increase future rebase drag
- does it preserve the hard-replacement target
- does it accidentally introduce split-brain behavior
- does it move the system toward maximum-performance agentmemory usage rather than a degraded fallback
```

## Do not do

- Do not run Codex native memory injection and `agentmemory` injection as
  equal peers long term.
- Do not claim a strict superset without rebuilding missing Codex-native
  semantics.
- Do not clone Claude plugin infrastructure into Codex just to make the
  replacement work.
- Do not overfit to Claude-specific bridge behavior such as
  `~/.claude/projects/*/memory/MEMORY.md` if Codex is becoming the primary
  target.
- Do not remove native memory citations or memory operations accidentally; if
  they are dropped, document that as an intentional product change.

## Acceptance criteria for a forked replacement

The replacement is successful only if all of these are true:

- `agentmemory` is the authoritative source for retrieved memory context,
- Codex native memory is no longer an independent competing authority,
- Codex startup injection still works reliably,
- memory refresh and memory clearing remain user-visible operations or are
  intentionally removed with docs,
- hook/event coverage is sufficient to produce materially useful observations,
- token usage stays bounded as the corpus grows,
- the intended steady state uses embeddings and hybrid retrieval rather than a
  degraded BM25-only baseline,
- Gemini or another high-quality embedding provider remains available as a
  first-class configuration path,
- the fork has a clear provenance story for memory-derived output,
- there is no ambiguity about which memory system is active,
- the resulting user-facing behavior is a practical superset of the two source
  systems rather than a regression-heavy swap.

## Final judgment

If the question is "is `agentmemory` materially more advanced than Codex
native memory?", the answer is yes.

If the question is "should a fork disable Codex native memory and replace it
with `agentmemory`?", the answer is:

- yes,
- with the condition that the fork also rebuild the Codex-native integration
  semantics that matter,
- and with the explicit goal of a single authoritative memory system rather
  than a permanent hybrid.
