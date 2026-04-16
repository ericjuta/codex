# Claude Code Hook Parity Spec

## Status

Detailed implementation contract for Claude Code plugin parity in this fork.

This document defines what "full parity with the Claude Code plugin setup"
means for the Codex fork when `agentmemory` is the active memory backend.

It is intentionally narrower than:

- [`fork-intent.md`](./fork-intent.md)
- [`../codex-rs/docs/agentmemory_runtime_surface_spec.md`](../codex-rs/docs/agentmemory_runtime_surface_spec.md)

Current branch state:

- `SessionStart` registration uses `POST /agentmemory/session/start`
- `PreToolUse` enrichment uses `POST /agentmemory/enrich` and accepts
  `additionalContext`
- native observe payloads now declare `source = codex-native`,
  `payload_version = 1`, a stable `event_id`, sender `capabilities`, and an
  explicit `persistence_class`
- native `PostToolUse` and `PostToolUseFailure` observations now normalize to
  `tool_input` / `tool_output` / `error` instead of the older
  `command` / `tool_response` sender shape
- non-shell post-tool capture now covers the same native tool-name lane as the
  enrichment matcher when Codex emits `Edit|Write|Read|Glob|Grep`
- `UserPromptSubmit` retains `observe` and now issues
  `POST /agentmemory/context/refresh` for long-enough prompts
- shutdown emits `Stop` observation plus summarize, then `SessionEnd`
  observation plus `session/end`, but bare shutdown markers are now classified
  as non-persistent sender diagnostics instead of ordinary observations
- when consolidation is enabled, shutdown also issues
  `POST /agentmemory/crystals/auto` and
  `POST /agentmemory/consolidate-pipeline`
- current sender capabilities still do not advertise `assistant_result`;
  freshness remains stop/task-completed driven on the Codex side

Those documents define the memory backend and the runtime memory surface at a
higher level. This document defines the hook-level operator contract that must
match the Claude plugin setup.

## Goal

An operator who already knows the `agentmemory` Claude Code plugin setup should
be able to point the forked Codex runtime at the same backend and get the same
practical behavior for:

- session-start context injection
- pre-tool enrichment on file/search tools
- prompt-submit context refresh
- hook environment variables
- default-off token-cost posture
- observation capture and lifecycle side effects for the rest of the session

Parity here means behavioral parity, not merely naming similarity.

## Canonical Reference Behavior

The canonical reference for parity is the Claude plugin setup shipped in the
`agentmemory` repository:

- `plugin/.claude-plugin/hooks.json`
- `src/hooks/session-start.ts`
- `src/hooks/pre-tool-use.ts`

At the time of this spec, that contract is:

1. `SessionStart`
   - always calls `POST /agentmemory/session/start`
   - only injects returned `context` when `AGENTMEMORY_INJECT_CONTEXT=true`
2. `PreToolUse`
   - is a no-op unless `AGENTMEMORY_INJECT_CONTEXT=true`
   - only enriches `Edit|Write|Read|Glob|Grep`
   - calls `POST /agentmemory/enrich`
   - injects returned `context` into the upcoming model turn
3. `UserPromptSubmit`
   - always calls `POST /agentmemory/observe`
   - calls `POST /agentmemory/context/refresh` when the submitted prompt is
     long enough to justify query-aware retrieval
   - remains outside the `SessionStart`/`PreToolUse` injection lane, but is
     still part of full plugin parity
4. `Stop`
   - always calls `POST /agentmemory/observe`
   - always calls `POST /agentmemory/summarize`
5. `SessionEnd`
   - always calls `POST /agentmemory/session/end`
   - when consolidation is enabled, also calls:
     - `POST /agentmemory/crystals/auto`
     - `POST /agentmemory/consolidate-pipeline`
6. `PostToolUse`, `PostToolUseFailure`, `PreCompact`, `SubagentStart`,
   `SubagentStop`, `Notification`, and `TaskCompleted`
   - are capture-oriented hooks
   - are not part of the context-injection path

This fork must match that contract unless this document explicitly says
otherwise.

## Decision

The fork should implement full Claude plugin parity for `agentmemory` context
injection.

That means:

1. Codex must use the same hook events for injection:
   - `SessionStart`
   - `PreToolUse`
2. Codex must use the same backend endpoints for injection:
   - `POST /agentmemory/session/start`
   - `POST /agentmemory/enrich`
3. Codex must expose a config-first operator surface in `config.toml` for
   parity-related settings.
4. Codex must continue to support the Claude-compatible environment variable
   names as override and compatibility inputs.
5. Context injection must remain default-off unless the operator explicitly
   enables it.
6. The assistant-facing `memory_recall` tool remains a separate runtime
   surface and does not replace hook injection.

## Current State

The parity lane is implemented on this branch.

What remains valuable here is keeping the specification synchronized with the
actual operator contract so future edits do not silently regress the behavior
above.

## Required Runtime Contract

### Backend Gate

The parity path applies only when:

- `config.memories.backend == Agentmemory`

When the backend is `native`, Codex must not call the `agentmemory` injection
endpoints.

### Feature Gate

The parity path must not depend on `Feature::MemoryTool`.

Rationale:

- the Claude plugin injection path is hook-driven, not assistant-tool-driven
- operators should be able to enable Claude-style injection without separately
  opting into the assistant-facing recall tool
- `memory_recall` may remain gated by `Feature::MemoryTool`; hook injection may
  not

### Config Contract

The primary operator surface for this fork must be `~/.codex/config.toml`, not
an env-only setup.

Recommended shape:

```toml
[memories]
backend = "agentmemory"

[features]
memory_tool = true

[memories.agentmemory]
base_url = "http://127.0.0.1:3111"
inject_context = true
secret_env_var = "AGENTMEMORY_SECRET"
```

Required semantics:

- `memories.backend = "agentmemory"`
  - selects the `agentmemory` backend
- `memories.agentmemory.base_url`
  - primary backend base URL for parity-related requests
- `memories.agentmemory.inject_context`
  - boolean gate for startup and pre-tool injection
- `memories.agentmemory.secret_env_var`
  - names the environment variable whose value should be used as bearer auth
    when present
- `memories.use_memories`
  - is the current Codex-native toggle for the optional shutdown-side
    `crystals/auto` plus `consolidate-pipeline` behavior

Design rules:

- `base_url` belongs in `config.toml`
- the injection enablement flag belongs in `config.toml`
- the secret value itself should not be stored directly in `config.toml`
- the assistant-facing `memory_recall` tool may continue to use
  `[features].memory_tool`

### Environment Compatibility Contract

Codex must still honor these Claude-compatible environment variable names:

- `AGENTMEMORY_URL`
- `AGENTMEMORY_SECRET`
- `AGENTMEMORY_INJECT_CONTEXT`

But they are not the primary configuration surface for this fork.

Required precedence:

1. explicit Claude-compatible env vars
2. `config.toml` parity settings
3. implementation default

Required semantics:

- `AGENTMEMORY_URL`
  - overrides `memories.agentmemory.base_url`
- `AGENTMEMORY_INJECT_CONTEXT`
  - string `"true"` enables injection
  - string `"false"` disables injection
  - overrides `memories.agentmemory.inject_context`
- `AGENTMEMORY_SECRET`
  - overrides `memories.agentmemory.secret_env_var` indirection
- `CONSOLIDATION_ENABLED`
  - when set to `"true"` or `"false"`, overrides the shutdown-side
    consolidation toggle so Codex matches the standalone `agentmemory` hook
    runtime

Rationale:

- `config.toml` is the cleanest persistent operator surface in Codex
- Claude-compatible env names preserve launcher compatibility and ad hoc runs
- secret indirection matches existing Codex patterns better than storing a raw
  secret value in `config.toml`

## SessionStart Parity

### Required Behavior

On `SessionStart`, Codex must:

1. call `POST /agentmemory/session/start`
2. send:
   - `sessionId`
   - `project`
   - `cwd`
3. continue startup even if the request fails or times out
4. only inject returned `context` when context injection is enabled by the
   config/env precedence rules above

### Injection Semantics

When injection is enabled and the backend returns non-empty `context`, Codex
must inject that context into the first model turn as developer-context input.

Equivalent behavior is acceptable. Exact transport is not mandated as long as:

- the injected text reaches the same turn that starts the session
- it is available to the model before the first assistant response
- it is not delayed until after the turn completes

### Non-Goals

This parity lane must not keep the current synthetic startup retrieval path as
the primary mechanism.

Specifically:

- `POST /agentmemory/context` with `sessionId = "startup"` is not the
  canonical parity path
- if retained for fallback, it must be documented as fallback only and must not
  change the primary Claude-parity behavior

## PreToolUse Parity

### Required Behavior

On `PreToolUse`, Codex must behave like the Claude plugin setup.

When context injection is disabled:

- the enrichment path must be a no-op

When context injection is enabled:

- only these tools are eligible:
  - `Edit`
  - `Write`
  - `Read`
  - `Glob`
  - `Grep`
- Codex must derive `files` and `terms` from the tool payload using the same
  intent as the plugin hook
- Codex must call `POST /agentmemory/enrich`
- the request must include:
  - `sessionId`
  - `files`
  - `terms`
  - `toolName`
- non-empty returned `context` must be injected into the upcoming model/tool
  turn before execution continues

### Parser Requirement

Codex hook parsing must accept `additionalContext` on `PreToolUse` for the
parity path.

This is mandatory. Rejecting `additionalContext` on `PreToolUse` is not
compatible with Claude plugin parity.

### Scope Rule

Do not broaden the pre-tool injection matcher beyond the Claude plugin set in
this lane.

In particular:

- do not inject on shell/exec tools in the initial parity implementation
- do not inject on every tool by default

If the fork later wants broader enrichment, that belongs in a new spec, not in
the Claude parity lane.

## UserPromptSubmit Parity

### Required Behavior

`UserPromptSubmit` remains a capture-oriented hook, but full plugin parity
still requires its current side effects.

Codex must:

1. call `POST /agentmemory/observe`
2. send the prompt payload in Claude-compatible shape
3. when the submitted prompt is long enough to justify refresh, call
   `POST /agentmemory/context/refresh`
4. inject returned `context` when the backend returns non-empty context and the
   backend does not mark the request as skipped

Current implementation rule:

- prompt-submit refresh is independent of
  `memories.agentmemory.inject_context` and follows the plugin's
  prompt-length-plus-backend-skip contract instead

Design rule:

- `UserPromptSubmit` is not a substitute for `SessionStart` or `PreToolUse`
  injection, but it is still part of the current plugin contract

## Stop And SessionEnd Parity

### Stop

Codex must:

1. call `POST /agentmemory/observe`
2. call `POST /agentmemory/summarize`

This is required parity behavior, not an optional enhancement.

Sender-side payload quality rule:

- a real turn stop may remain useful lifecycle input
- a synthetic shutdown stop with only `session_id` / `cwd` must be classified
  as `diagnostics_only`, not as a normal persistent memory observation

### SessionEnd

Codex must:

1. call `POST /agentmemory/session/end`
2. when the parity-equivalent consolidation mode is enabled, also call:
   - `POST /agentmemory/crystals/auto`
   - `POST /agentmemory/consolidate-pipeline`

The exact config knob may be Codex-native, but the behavior must remain
equivalent to the plugin's enabled path.

Current implementation note:

- Codex currently uses `memories.use_memories` as the native toggle and still
  honors `CONSOLIDATION_ENABLED` as an override for parity with the standalone
  hook runtime
- the native `SessionEnd` observe payload now carries summarize outcome fields
  and emits as `ephemeral` instead of an unclassified bare lifecycle marker

## Observation Parity

This spec is about context injection parity, but the surrounding hook capture
must remain aligned with the plugin setup.

When `config.memories.backend == Agentmemory`, Codex should continue to capture
the broader lifecycle hooks already modeled in the fork:

- `UserPromptSubmit`
- `PostToolUse`
- `PostToolUseFailure`
- `Stop`
- `SessionEnd`
- `SubagentStart`
- `SubagentStop`
- `Notification`
- `TaskCompleted`

These are observation hooks, not context-injection hooks, and should not be
repurposed as substitutes for `SessionStart` or `PreToolUse` injection.

Current Codex-owned concrete coverage in this repo is:

- `apply_patch` mapped to `Edit|Write`
- `list_dir` mapped to `Glob`
- generic native tool invocations already named `Edit|Write|Read|Glob|Grep`
  now forward structured arguments into the same observe/enrich contract

### Explicit Deferral

This lane does not require adding new non-injection hook families solely to say
"Claude has them too".

In particular:

- `PreCompact` is not required for context-injection parity
- if the fork later adds `PreCompact` parity, it should do so in a separate
  hook-lifecycle spec with its own payload and UX contract

## Operator Experience

Full parity means an operator can move from the Claude plugin setup to the
forked Codex runtime without learning a different injection model.

Minimum operator guarantees:

- a normal persistent setup can be done entirely in `~/.codex/config.toml`
  except for the secret value
- the secret can be provided indirectly via `secret_env_var`
- the same Claude-compatible env names still work as overrides
- leaving both config and env injection flags unset keeps injection off
- file/search tool enrichment only happens when the operator explicitly opted in

The fork may use Codex-native config fields for the primary operator surface,
but it must not require a fork-specific environment variable vocabulary.

## Acceptance Criteria

This lane is complete only when all of the following are true.

### Behavior

- `SessionStart` registration still happens when `backend == Agentmemory`
- `SessionStart` injects returned context only when context injection is
  enabled by config/env precedence
- `PreToolUse` injects returned context only when context injection is enabled
  by config/env precedence
- `PreToolUse` injection is limited to `Edit|Write|Read|Glob|Grep`
- `PreToolUse` accepts `additionalContext`
- `UserPromptSubmit` still calls `observe`
- `UserPromptSubmit` uses `/agentmemory/context/refresh` for query-aware
  refresh when appropriate
- native observe payloads include explicit sender metadata:
  `source`, `payload_version`, `event_id`, `capabilities`, and
  `persistence_class`
- native post-tool observe payloads normalize to
  `tool_input` / `tool_output` / `error`
- `Stop` still calls `observe`
- `Stop` still calls `summarize`
- `SessionEnd` still calls `session/end`
- when consolidation is enabled, `SessionEnd` also calls `crystals/auto` and
  `consolidate-pipeline`
- `memory_recall` remains available only on the assistant tool surface when its
  existing feature/backend gates are satisfied
- `memories.agentmemory.base_url` controls the backend URL when no override env
  var is present
- `memories.agentmemory.inject_context` controls injection when no override env
  var is present
- `memories.agentmemory.secret_env_var` is used to resolve bearer auth when no
  direct secret override env var is present

### Tests

At minimum, add or update tests that prove:

- startup injection uses `/agentmemory/session/start`, not only
  `/agentmemory/context`
- startup injection follows `config.toml` injection enablement when no override
  env var is set
- pre-tool enrichment injects context for `Edit|Write|Read|Glob|Grep`
- pre-tool enrichment does not inject for non-matching tools
- `PreToolUse` parsing accepts valid `additionalContext`
- native post-tool capture covers the real non-shell sender lane Codex owns
- prompt-submit parity keeps `observe` and `context/refresh` wired together
- stop parity keeps `observe` and `summarize` wired together
- session-end parity keeps `session/end` wired and, when enabled,
  `crystals/auto` plus `consolidate-pipeline`
- `memories.agentmemory.base_url` is honored by the parity path
- `memories.agentmemory.inject_context` is honored by the parity path
- `memories.agentmemory.secret_env_var` resolves auth correctly
- `AGENTMEMORY_URL`, `AGENTMEMORY_SECRET`, and `AGENTMEMORY_INJECT_CONTEXT`
  override `config.toml` when present
- `Feature::MemoryTool = false` does not disable the hook-based parity path

### Documentation

Update runtime-facing docs so they say:

- Codex supports Claude-style parity with a config-first operator surface
- persistent setup belongs in `~/.codex/config.toml`
- env vars remain compatible overrides
- startup injection comes from `session/start`
- pre-tool enrichment comes from `enrich`
- prompt-submit refresh comes from `context/refresh`
- native observe payloads are explicitly versioned and attributed
- bare shutdown lifecycle markers are sender-classified as non-persistent
- stop still summarizes the session
- session end still closes the session and, when enabled, runs the same
  maintenance calls as the plugin
- `memory_recall` is complementary, not a replacement for hook injection

## Explicit Non-Goals

This spec does not require:

- exposing destructive memory tools to the assistant
- broadening pre-tool injection to shell commands
- changing the native memory backend
- replacing the existing `memory_recall` tool contract

## Related Docs

- [`fork-intent.md`](./fork-intent.md)
- [`../codex-rs/docs/agentmemory_runtime_surface_spec.md`](../codex-rs/docs/agentmemory_runtime_surface_spec.md)
