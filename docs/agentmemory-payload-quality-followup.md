# Codex-Agentmemory Payload Follow-up Spec

## Purpose

Audit current `codex` sender behavior after the payload-quality landing and scope
only the work that still remains.

This document is intentionally narrower than
[`agentmemory-payload-quality.md`](./agentmemory-payload-quality.md).

That earlier document captured the full sender-side target state. Much of it is
now implemented. This follow-up spec exists to avoid re-opening already-closed
lanes and to focus on optional hardening.

## Status

The required follow-up work in this document is implemented on this branch.

What remains after this change is optional hardening only:

- shared cross-repo compatibility fixtures or corpus generation

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
  - `SessionEnd` emits `ephemeral` when it only carries closeout status
- `TaskCompleted`, `SubagentStop`, and `Notification` now require explicit
  `cwd` through the sender normalization path
- query-aware retrieval is wired:
  - prompt submit calls `/agentmemory/context` with prompt-derived `query`
  - fallback recall also calls `/agentmemory/context`
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
  - real `AssistantResult` emit

## Required Follow-up Work Closure

### 1. Real `AssistantResult` events

`Codex` now emits `capture_event("AssistantResult", ...)` for final assistant
text, so freshness is no longer stop-only.

### 2. `assistant_result` capability advertised

`NATIVE_OBSERVE_CAPABILITIES` includes `assistant_result`.

### 3. End-to-end sender test coverage

An end-to-end test in
`codex-rs/core/tests/suite/agentmemory_hook_parity.rs`
asserts the emitted `AssistantResult` request body.

## Optional Hardening

### Cross-repo compatibility corpus

This is not required to unlock rollout, but it is the best anti-drift follow-up.

#### Problem

Current sender tests assert Codex request bodies against mocked endpoints, and
receiver tests assert agentmemory ingest behavior against synthetic/native payloads.
That is good, but the two repos still rely on mirrored assumptions instead of a
shared compatibility corpus.

#### Possible follow-up

Introduce a small shared JSON fixture corpus or mirrored fixture generation so
both repos pin the same supported native wire shapes.

#### Not a blocker

Do not block rollout on this.

## Explicitly Not Needed Right Now

- no `payload_version` bump
- no new persistence classes
- no further shutdown-hygiene work
- no reopen of the old post-tool normalization lane
- no additional receiver-side work in `agentmemory` unless sender rollout finds
  a real mismatch
- no `sequence` metadata lane until a concrete consumer exists

## Recommended Change Order

1. add cross-repo fixture hardening only
2. keep behavior and tests synchronized as agentmemory evolves

## Standard Of Done

This follow-up lane is done when optional hardening is completed.
