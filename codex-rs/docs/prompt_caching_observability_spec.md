# Prompt Caching Observability and Stabilization Spec

Status: measurement-first design, based on the July 12, 2026 prompt-cache audit.

This spec defines how Codex should measure provider prompt-cache behavior before
changing prompt assembly, tool ordering, or cache-key scope. The immediate goal
is to identify repeatable client-side miss causes with provider-reported evidence.
The implementation should remain incremental, bounded, privacy-safe, and
compatible with compaction, resume, fork, and transport reuse.

## Decision

Do not change prompt assembly or the default cache key from this spec alone.
The current code has stable cache-key and incremental-context behavior, but the
repository does not contain a production baseline that connects request shape
to provider cache hits, latency, or effective input cost.

The first implementation slice is a request-level observation ledger containing
bounded digests and provider usage. A later change is justified only when the
ledger shows a repeated miss pattern and the provider's cached-token counts
show material input-token impact.

## Goals

- Measure cached input tokens, total input tokens, cache ratio, and TTFT by
  request class.
- Distinguish a changed cache surface from a transport or provider miss when
  the model-visible request is otherwise stable.
- Attribute request changes to instructions, tools, input-prefix items,
  settings/context updates, world-state updates, or rebuild boundaries without
  recording raw prompts or tool arguments.
- Compare normal HTTP requests, WebSocket reuse, warmup, retries, compaction,
  resume, fork, guardian, subagent, MCP, skill, and plugin paths.
- Preserve the existing incremental history model and all context size caps.
- Produce a small, evidence-backed implementation slice rather than a broad
  prompt rewrite.

## Non-Goals

- Do not broaden the default prompt cache key beyond the thread identity.
- Do not treat a stable cache key as proof that instructions or tools are
  unchanged.
- Do not move volatile time, environment, recommendation, or runtime state into
  a stable prefix merely to increase its apparent size.
- Do not record raw prompts, instructions, tool schemas, tool arguments, file
  paths, environment values, secrets, account identifiers, or response text in
  production telemetry.
- Do not use high-cardinality request or digest values as metric labels.
- Do not infer provider cache behavior from mocked cached-token fixtures alone.
- Do not add runtime configuration until measurement identifies a concrete need.

## Current Evidence and Source Surfaces

| Surface | Current behavior | Audit implication |
| --- | --- | --- |
| Cache key | `ModelClient` uses the thread id by default. Guardian review sessions use a key scoped to the parent thread. | Keep the scope unchanged until cross-thread sharing is explicitly required and proven safe. |
| WebSocket reuse | Reuse compares model, instructions, tools, tool choice, reasoning, include, service tier, cache key, text, and related request properties. It intentionally handles input separately and ignores client metadata. | Request equality and input-prefix equality need separate observations. |
| MCP tools | MCP discovery collects server tools, normalizes names, and sorts candidates by raw tool identity. `StepContext` snapshots the list for one sampling request. | Ordinary MCP ordering is already protected; do not add a second sort without evidence. |
| Other tools | Core, dynamic, hosted, and extension tools are assembled in source/registration order. Namespace members are sorted, while top-level namespace placement preserves first appearance. | Dynamic or extension ordering remains a plausible but unproven miss cause. |
| Context history | Steady-state turns append settings and world-state diffs. Full initial context is rebuilt at selected boundaries. | Rebuild paths need stronger replay evidence than normal follow-up turns. |
| Rebuild completeness | `build_settings_update_items` documents that it does not cover every model-visible item emitted by initial context construction. | Fork, resume, compaction, model switch, and extension refresh are audit priorities. |
| Time reminders | Current-time reminders are appended as contextual user items when due. | They should remain an explicit delta and must not destabilize the stable prefix. |
| World state | Built-in sections use stable snapshots and diffs; extension sections follow contributor order. | Compare section-level digests and contributor order during rebuilds. |
| Provider usage | Responses usage already parses `input_tokens_details.cached_tokens`; session and analytics paths retain cached input-token counts. | Provider usage is the hit-rate truth surface. |
| Rollout trace | Enabled inference tracing stores full request payloads and response summaries. | Useful for local forensic replay, but too raw for a production cache ledger. |

## Observation Plan

### Two telemetry planes

Use separate planes so debugging does not turn high-cardinality request identity
into a production metric dimension.

1. **Aggregate analytics:** low-cardinality request class, model/provider,
   transport, outcome, input tokens, cached input tokens, output tokens, and
   TTFT.
2. **Local or sampled ledger:** bounded request-surface digests, prefix
   comparison fields, context transition, and transport reuse details. This
   plane is for attribution and must never contain raw model-visible values.

### Required observation fields

| Field | Plane | Requirement |
| --- | --- | --- |
| `request_class` | Both | A bounded enum such as `normal`, `tool_heavy`, `mcp`, `settings_change`, `compaction`, `resume`, `fork`, `guardian`, `subagent`, `prewarm`, or `retry`. |
| `model` and `provider` | Aggregate | Use existing stable model/provider identifiers. |
| `transport` | Aggregate | `http`, `websocket`, `websocket_reused`, `warmup`, or `fallback`. |
| `prompt_cache_key_scope` | Aggregate | `thread`, `guardian_parent`, or another bounded scope name; never emit the raw key. |
| `prompt_cache_key_digest` | Ledger | A keyed, domain-separated digest for comparing requests without exposing the key. |
| `instructions_digest` | Ledger | Digest of the final serialized instructions after request preparation. |
| `tools_digest` | Ledger | Digest of the final ordered model-visible tools array. |
| `tools_set_digest` | Ledger | Optional order-insensitive diagnostic digest to distinguish reorder from schema/content change. |
| `input_prefix_digest` | Ledger | Digest of the ordered input item sequence used for the request. |
| `first_divergent_input_index` | Ledger | Longest common prefix comparison against the immediately previous comparable request, or `unknown`. |
| `input_item_count` and `context_transition` | Both | Bounded counts and values such as `initial`, `delta`, `world_state_delta`, `compaction_rebuild`, or `unknown`. |
| `input_tokens` and `cached_input_tokens` | Aggregate | Copy provider-reported usage; do not estimate cache hits from client hashes. |
| `cache_ratio` | Aggregate | `cached_input_tokens / input_tokens` when input tokens are positive. |
| `ttft_ms` | Aggregate | Existing time-to-first-token measurement when available. |
| `previous_response_id_present` | Aggregate | Indicates whether provider-side response continuation was available. |
| `ledger_truncated` | Ledger | Required when bounded digest comparison reaches its cap. |

Digest values must be generated by one domain-separated cryptographic digest
helper. The algorithm and salt storage belong to the implementation slice, but
the raw values must never be serialized beside the digest. The ledger must cap
item-level comparison metadata, for example at 256 input items and a fixed
serialized observation budget, and set `ledger_truncated` when the cap is hit.

### Cache-surface comparison

Compare the request in two layers:

1. **Stable request properties:** model, instructions, tools, tool choice,
   parallel tool calls, reasoning, store/stream controls, include, service tier,
   prompt cache key, and text controls.
2. **Ordered input sequence:** the model-visible input items, compared by
   bounded per-item digests and longest common prefix.

The comparison must use the final request representation after item preparation.
It must preserve input order and must not normalize away differences that the
provider can observe. Tool set and tool order should be reported separately so
that a future canonicalization change is only made when order is the observed
cause.

## Controlled Measurement Matrix

Run a synthetic mocked lane first, followed by a bounded live lane with only
the aggregate fields and sampled ledger digests enabled.

| Class | Controlled change | Primary evidence |
| --- | --- | --- |
| Normal follow-up | New user item only | Stable request properties and input-prefix extension |
| Tool-heavy | Repeated tool calls with stable schemas | Tool digest, input divergence, cached tokens |
| MCP | Stable MCP list, refresh, discovery, and search | Ordered tool digest and MCP snapshot behavior |
| Skills/plugins | Load, unload, or refresh one contributor | Instructions/context/tool digests and contributor order |
| Settings/model | Change one setting or model between turns | Expected delta versus unintended prefix rebuild |
| Compaction | Compact and continue | Rebuild boundary, first divergence, cached tokens |
| Resume/fork | Resume and fork equivalent histories | Replayed context completeness and cache-key scope |
| Guardian/subagent | Reuse same parent and use a different parent | Guardian key scope and stable prompt surface |
| Transport | HTTP, reused WebSocket, warmup, and fallback | Reuse state versus provider cache outcome |

The report for each class must include sample count, median and p95 TTFT,
input-token volume, cached-token volume, cache ratio, request-surface change
rate, and the top observed divergence category. If provider cached-token data
is unavailable, report the lane as unmeasured rather than estimating it.

## Decision Gates

| Observation | Follow-up |
| --- | --- |
| Stable request surface but low cached tokens | Investigate provider thresholds, eviction, or transport behavior; do not rewrite prompt assembly. |
| Same tool set, different ordered tools array | Add a narrowly scoped canonical-order change and regression coverage. |
| Tool schema/content changes | Fix the contributor or snapshot boundary that changes the schema; sorting alone is insufficient. |
| Unexpected common-prefix change in steady state | Fix the emitting context update or volatile-fragment placement. |
| Missing model-visible content after rebuild | Persist the missing dependency or add an explicit replay event before changing cache strategy. |
| Stable cache key with cross-thread contamination risk | Preserve isolation; never widen the key as an optimization shortcut. |
| No repeatable client-side cause | Stop after measurement and retain the current assembly behavior. |

## Implementation Stages

### Stage 1: ledger and synthetic proof

- Add a request observation type at the model-client boundary.
- Compute bounded digests after request preparation and before transport send.
- Record provider usage and TTFT at completion or failure.
- Add mocked tests that vary exactly one stable property, tool order, input
  item, context delta, transport path, and rebuild boundary.
- Keep ledger output local or explicitly sampled; do not add digest labels to
  metrics.

### Stage 2: controlled live baseline

- Run the measurement matrix against a provider path that reports cached input
  tokens.
- Compare cache ratios and TTFT by class and transport.
- Retain only aggregate results and redacted, bounded attribution data in the
  report.

### Stage 3: smallest confirmed fix

Implement only the confirmed cause:

- canonicalize final tool order if repeated order-only misses are measured;
- freeze or persist a missing snapshot if a rebuild loses model-visible state;
- keep volatile environment, time, recommendation, and runtime updates as
  appended deltas;
- add regression coverage for the affected request class.

Each fix must preserve the default thread cache key, input ordering semantics,
context bounds, compaction behavior, and guardian isolation.

## Verification Requirements

- Existing `codex-core` prompt-caching tests remain green.
- Request equality tests remain exhaustive when request fields change.
- Tool assembly tests cover both ordered equality and order-only diagnostics.
- Resume, fork, compaction, and guardian tests assert cache-key scope and
  model-visible context continuity.
- Ledger tests prove raw prompts, instructions, tools, arguments, and secrets
  are absent from serialized observations.
- All observation collections have explicit size caps.
- The final report distinguishes provider measurements, client observations,
  and inference.

## Success Criteria

This spec is complete when the team can answer, from provider-backed data:

1. Which turn classes have the lowest cached-input ratio?
2. Whether misses are caused by changed instructions, tools, input prefixes,
   rebuilds, transport reuse, or provider behavior.
3. How many input tokens and how much TTFT are affected by each repeatable
   client-side cause.
4. Whether a narrowly scoped code change is justified.

Until those questions are answered, the correct outcome is measurement and no
prompt-assembly change.
