# Codex-Agentmemory Payload Follow-up Spec

## Purpose

Audit current `codex` sender behavior after the payload-quality landing and
scope only the work that still remains.

This document is intentionally narrower than
[`agentmemory-payload-quality.md`](./agentmemory-payload-quality.md).

That earlier document captured the full sender-side target state. Much of it is
now implemented. This follow-up spec exists to avoid re-opening already-closed
lanes and to focus the next change set on the actual remaining sender gaps.

## Audit Result

As of this audit, the following sender-side lanes are already landed in
`codex`:

- native observe payloads are normalized through
  `codex-rs/core/src/agentmemory/observe_payload.rs`
- observe payloads now include:
  - `source = "codex-native"`
  - `payload_version = "1"`
  - stable `event_id`
  - `capabilities`
  - explicit `persistence_class`
- `PostToolUse` and `PostToolUseFailure` now normalize to:
  - `tool_input`
  - `tool_output`
  - `error`
- shutdown hygiene is fixed:
  - bare `Stop` emits `diagnostics_only`
  - `SessionEnd` emits `ephemeral` when it only carries summarize status
- `TaskCompleted`, `SubagentStop`, and `Notification` now require explicit
  `cwd` through the sender normalization path
- query-aware retrieval is wired:
  - prompt submit calls `/agentmemory/context/refresh`
  - fallback recall calls `/agentmemory/context`
  - runtime recall preserves `query`
- pre-tool enrichment covers the native file/search lanes already under
  `agentmemory` handling
- non-shell post-tool capture exists for the current minimum native lane:
  - `Edit`
  - `Write`
  - `Read`
  - `Glob`
  - `Grep`
- sender-side parity tests exist for:
  - normalized post-tool payloads
  - refresh-to-context fallback
  - shutdown lifecycle observe payloads

## Remaining Required Work

### 1. Emit real `AssistantResult` events

#### Problem

`observe_payload.rs` supports `AssistantResult`, but current runtime code does
not actually emit `capture_event("AssistantResult", ...)`.

So freshness on the Codex side is still effectively driven by:

- `Stop`
- `TaskCompleted`

instead of a true final assistant-result lane.

#### Required outcome

Codex should emit a real `AssistantResult` native observe payload when a turn
produces final assistant text.

#### Minimum payload

- `session_id`
- `turn_id`
- `cwd`
- `model`
- `assistant_text`
- `is_final = true`

#### Likely implementation seam

The emission point should be attached to the existing finalized assistant-text
path, not bolted onto a second independent text reconstruction path.

Primary candidates:

- `codex-rs/core/src/codex.rs`
  - assistant-item completion flow
  - finalized assistant message extraction
  - response completion flush path

#### Acceptance criteria

- exactly one final `AssistantResult` observe event is emitted per finalized
  assistant message item or per turn completion path, depending on the chosen
  ownership model
- emitted `assistant_text` matches the same user-visible final assistant text
  already used by the runtime
- the event includes explicit `cwd` and `model`
- the new event does not duplicate `Stop` semantics or create multiple
  conflicting freshness records for the same turn

### 2. Advertise `assistant_result` capability when the lane is real

#### Problem

Current sender capabilities intentionally do not advertise
`assistant_result`.

That is currently honest, but once the event is emitted the capability list
must be updated or the contract becomes misleading.

#### Required outcome

When `AssistantResult` emission lands, add `assistant_result` to the native
capability list and update the parity docs and tests in the same change.

#### Acceptance criteria

- `NATIVE_OBSERVE_CAPABILITIES` includes `assistant_result`
- suite expectations that inspect `capabilities` are updated
- docs stop saying freshness is stop-driven only

### 3. Add one end-to-end sender test for `AssistantResult`

#### Problem

Current suite covers post-tool and shutdown lanes well, but not an actual
runtime-emitted `AssistantResult` request body.

#### Required outcome

Add one end-to-end test that boots a Codex session against a mock
`agentmemory` server and proves the actual `AssistantResult` request body.

#### Acceptance criteria

- test captures `/agentmemory/observe`
- test asserts:
  - `hookType = "assistant_result"`
  - `source = "codex-native"`
  - `payload_version = "1"`
  - stable `event_id`
  - `persistence_class = "persistent"`
  - final `assistant_text`
  - `is_final = true`

## Optional Hardening

### Cross-repo compatibility corpus

This is not required to unlock rollout, but it is the best anti-drift follow-up.

#### Problem

Current sender tests assert Codex request bodies against mocked endpoints, and
receiver tests assert agentmemory ingest behavior against synthetic/native
payloads. That is good, but the two repos still rely on mirrored assumptions
instead of a shared compatibility corpus.

#### Possible follow-up

Introduce a small shared JSON fixture corpus or mirrored fixture generation so
both repos pin the same supported native wire shapes.

#### Not a blocker

Do not block `AssistantResult` rollout on this.

## Explicitly Not Needed Right Now

- no `payload_version` bump
- no new persistence classes
- no further shutdown-hygiene work
- no reopen of the old post-tool normalization lane
- no additional receiver-side work in `agentmemory` unless sender rollout finds
  a real mismatch
- no `sequence` metadata lane until a concrete consumer exists

## Recommended Change Order

1. emit `AssistantResult`
2. advertise `assistant_result` in capabilities
3. add end-to-end sender coverage
4. refresh the older docs to mark the remaining gap closed
5. optionally add cross-repo fixture hardening later

## Standard Of Done

This follow-up lane is done when:

- Codex emits real native `AssistantResult` observations
- sender capabilities honestly advertise `assistant_result`
- one end-to-end suite test proves the emitted wire shape
- parity docs no longer say freshness is stop-driven only
