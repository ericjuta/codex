## Codex-Agentmemory Payload Quality Spec

### Goal

Define the sender-side contract for native Codex -> agentmemory integration so
the runtime is:

- high-signal for recall
- clean for observability
- low-noise for storage
- explicit about version and capability drift

This is primarily a Codex-owned spec because most of the current gaps are on
the emitting side, not the ingest side.

### Status Note

This document captures the full sender-side target state from before the main
payload-quality landing.

Much of the gap list below is now closed in code. For the narrower post-landing
audit and the actual remaining sender work, see
[`agentmemory-payload-quality-followup.md`](./agentmemory-payload-quality-followup.md).

### Ownership

Codex owns:

- payload normalization
- lifecycle event emission
- tool coverage
- event identity and ordering semantics
- project and cwd attribution
- sender-declared schema version and capabilities
- live end-to-end contract verification from the Codex side

agentmemory owns:

- strict validation of the declared native contract
- persistence-class policy at ingest
- receiver-side compatibility tests

### Current Gaps

1. native post-tool payload shape does not match what agentmemory currently
   extracts
2. query-aware recall intent is dropped on the main runtime recall path
3. shutdown emits low-signal lifecycle junk as persistent observations
4. some secondary events omit explicit cwd and can be attributed to the wrong
   project
5. pre-tool enrichment coverage is only partial
6. non-shell post-tool capture is still missing
7. hook-type validation is too loose
8. event identity and ordering semantics are weak
9. schema negotiation is implicit instead of versioned
10. persistence classes are too blunt
11. current tests prove friendlier synthetic payloads more than the real native
    wire contract

### Core Principles

#### 1. Normalize at the boundary

Codex should emit one canonical agentmemory observation shape before calling
`/agentmemory/observe`.

#### 2. Do not store junk

Lifecycle events without turn identity, useful tool payload, user prompt, or
assistant conclusion should not become ordinary persistent observations.

#### 3. Freshness first, query aware when available

Query-aware ranking should help the main runtime recall path, but not overpower
same-session and recent-turn freshness.

#### 4. Version drift must be explicit

If native Codex payloads evolve, the wire contract should say so with explicit
metadata, not silent shape changes.

## Canonical Observation Contract

### Required top-level fields

- `sessionId`
- `hookType`
- `project`
- `cwd`
- `timestamp`
- `data`

### Required sender metadata

- `source`
  - required value for this lane: `codex-native`
- `payload_version`
  - explicit sender contract version
- `event_id`
  - stable per emitted lifecycle event

### Recommended sender metadata

- `source_timestamp`
  - original event timestamp if different from send-time timestamp
- `sequence`
  - per-session or per-turn monotonic sequence when available
- `capabilities`
  - sender-advertised optional support such as:
    - `assistant_result`
    - `structured_post_tool_payload`
    - `query_aware_context`
    - `event_identity`

### Required `data` fields by event family

#### `prompt_submit`

- `session_id`
- `turn_id`
- `cwd`
- `model`
- `prompt`

#### `post_tool_use`

- `session_id`
- `turn_id`
- `cwd`
- `model`
- `tool_name`
- `tool_use_id`
- `tool_input`
- `tool_output`

#### `post_tool_failure`

- `session_id`
- `turn_id`
- `cwd`
- `model`
- `tool_name`
- `tool_use_id`
- `tool_input`
- `error`

#### `assistant_result`

- `session_id`
- `turn_id`
- `cwd`
- `model`
- `assistant_text`
- `is_final`

#### `stop`

- `session_id`
- `turn_id`
- `cwd`
- `model`
- `last_assistant_message`

## Required Changes

### 1. PostToolUse and PostToolUseFailure Payload Convergence

#### Problem

Current native Codex capture sends `command` and `tool_response`, while
agentmemory extraction is currently oriented around `tool_input`,
`tool_output`, and `error`.

#### Required outcome

Codex must preserve structured input and structured result or error semantics
all the way into `/agentmemory/observe`.

#### Acceptance criteria

- native post-tool observations retain useful input/output/error payloads
- dedup can hash stable tool inputs
- synthetic compression has meaningful narrative and subtitle data
- turn capsules can extract files and concepts from real native post-tool
  payloads

### 2. Runtime Recall Query Propagation

#### Problem

Codex sends `query` to `/agentmemory/context`, but the current downstream path
can drop it before `mem::context`.

#### Required outcome

If Codex sends a query to `/agentmemory/context`, it must survive end to end.

#### Acceptance criteria

- runtime recall with query affects ranking predictably
- behavior without query stays materially unchanged
- prompt-submit `/agentmemory/context` and runtime recall semantics stay aligned

### 3. Shutdown Observation Hygiene

#### Problem

Codex shutdown can emit synthetic `Stop` and `SessionEnd` observations with
only `session_id` and `cwd`.

#### Required outcome

Bare shutdown lifecycle markers must not be stored as normal persistent
observations.

#### Allowed persistent cases

- stop tied to a real `turn_id`
- session-end payloads that intentionally carry meaningful summary-like data

#### Disallowed persistent cases

- stop with only `session_id` and `cwd`
- session-end with only `session_id` and `cwd`

#### Acceptance criteria

- shutdown no longer creates low-value compressed observations
- fake stop/session-end entries do not pollute recall or viewer surfaces

### 4. Project and CWD Attribution Correctness

#### Problem

Some secondary event families are emitted without explicit `cwd`, which allows
fallback to process cwd and weakens project attribution.

#### Minimum families to fix

- `TaskCompleted`
- `SubagentStop`
- `Notification`

#### Acceptance criteria

- every emitted native observation has explicit `cwd`
- project derivation never depends on process cwd fallback for supported native
  capture paths

### 5. Pre-Tool Enrichment Coverage Expansion

#### Problem

The enrich gate includes `Edit`, `Write`, `Read`, `Glob`, and `Grep`, but many
handlers still send no structured enrichment input.

#### Required outcome

All intended file/search tools in the enrich lane must send structured
`agentmemory_input`.

#### Acceptance criteria

- file/search tools covered by the gate actually reach `/agentmemory/enrich`
- enrichment remains skipped when there is no meaningful file/query signal
- docs describe actual coverage instead of aspirational coverage

### 6. Non-Shell Post-Tool Capture Coverage

#### Problem

Current post-tool observation capture is still effectively shell-only.

#### Required outcome

Codex should emit post-tool observations for the same high-signal native tool
families already treated as important by pre-tool enrichment.

#### Minimum lane

- `Edit`
- `Write`
- `Read`
- `Glob`
- `Grep`

#### Acceptance criteria

- non-shell file/search tools produce post-tool observations with useful result
  payloads
- post-tool observability is no longer shell-biased

### 7. AssistantResult Freshness Completeness

#### Problem

agentmemory supports `assistant_result`, but current native Codex review did
not confirm that the host emits it.

#### Required outcome

Preferred:

- Codex emits real `assistant_result` events with `turn_id` and final assistant
  text

Fallback:

- docs and tests explicitly state that current native freshness is stop-driven

#### Acceptance criteria

- if emitted, `assistant_result` updates turn capsules and working set
- if not emitted, docs stop overstating the current freshness path

### 8. Strict Hook-Type Validation

#### Problem

Unknown hook families can drift into storage too easily.

#### Required outcome

The native contract should define an explicit hook family set, and unsupported
types should fail clearly.

#### Acceptance criteria

- unsupported hook families are rejected instead of silently stored
- future expansion requires explicit schema and code updates

### 9. Event Identity, Ordering, and Source Semantics

#### Problem

Send-time timestamps alone are too weak for strong retry, dedup, and ordering
semantics.

#### Required outcome

Native observe payloads must carry explicit identity and source metadata.

#### Acceptance criteria

- retries can be deduplicated by identity instead of heuristics alone
- ordering can use source timestamps where available
- payload drift is versioned instead of silent

### 10. Schema Negotiation and Capability Signaling

#### Problem

Current native integration assumes implicit shared knowledge between sender and
receiver.

#### Required outcome

Codex must advertise native contract version and optional capability support.

#### Acceptance criteria

- agentmemory can branch on declared payload version when needed
- sender capability support is explicit in tests and diagnostics

### 11. Persistence Classes

#### Problem

Observation storage policy is too blunt for lifecycle-heavy native capture.

#### Required outcome

Codex-native integration should align with explicit persistence classes:

- `persistent`
- `ephemeral`
- `diagnostics_only`

#### Acceptance criteria

- non-recall lifecycle events do not automatically enter the same persistence
  lane as real memory-bearing observations
- docs define expected class per event family

## Verification

### Required repo-side tests

Add explicit tests for:

1. native post-tool payload normalization
2. post-tool failure payload normalization
3. query propagation through runtime recall
4. shutdown hygiene
5. missing cwd rejection or normalization
6. enrichment coverage for intended tool lanes
7. non-shell post-tool capture
8. hook-type allowlisting
9. event identity and source timestamp semantics
10. payload version and capability signaling
11. persistence-class behavior

### Required live verification

At least one end-to-end lane should prove what a real native Codex session
actually sends to:

- `/agentmemory/observe`
- `/agentmemory/context`
- `/agentmemory/enrich`
- `/agentmemory/session/start`
- `/agentmemory/session/closeout`

## Documentation

Related docs in this repo that should stay aligned:

- [claude-code-hooks-parity.md](./claude-code-hooks-parity.md)
- [fork-intent.md](./fork-intent.md)

The smaller receiver-side companion now lives in the `agentmemory` repo and
should stay scoped to ingest validation and storage policy rather than
re-specifying the sender contract.

## Standard Of Done

This lane is done when:

- native Codex post-tool observations retain useful input/output/error payloads
- runtime recall preserves query intent end to end
- shutdown junk no longer pollutes persistent observations
- all supported native capture payloads have stable cwd/project attribution
- enrichment and post-tool capture coverage match the intended native tool set
- unknown hook families are rejected
- native payloads carry explicit identity, version, and source semantics
- persistence class is explicit for non-recall lifecycle events
- compatibility tests pin the real supported wire shapes
- at least one live end-to-end verification path exists
- docs describe actual parity rather than aspirational parity
