# Claude Code Hook Parity Spec

## Status

Detailed implementation contract for Claude Code plugin parity in this fork.

This document defines what "full parity with the Claude Code plugin setup"
means for the Codex fork when `agentmemory` is the active memory backend.

It is intentionally narrower than:

- [`agentmemory-codex-memory-replacement-spec.md`](./agentmemory-codex-memory-replacement-spec.md)
- [`../codex-rs/docs/agentmemory_runtime_surface_spec.md`](../codex-rs/docs/agentmemory_runtime_surface_spec.md)

Those documents define the memory backend and the runtime memory surface at a
higher level. This document defines the hook-level operator contract that must
match the Claude plugin setup.

## Goal

An operator who already knows the `agentmemory` Claude Code plugin setup should
be able to point the forked Codex runtime at the same backend and get the same
practical behavior for:

- session-start context injection
- pre-tool enrichment on file/search tools
- hook environment variables
- default-off token-cost posture
- observation capture for the rest of the lifecycle

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
3. `UserPromptSubmit`, `PostToolUse`, `PostToolUseFailure`, `PreCompact`,
   `SubagentStart`, `SubagentStop`, `Notification`, `TaskCompleted`, `Stop`,
   and `SessionEnd`
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

## Current Gap

The fork is not currently at parity.

Current mismatches:

- startup injection currently uses `POST /agentmemory/context` with
  `sessionId = "startup"` instead of consuming `context` from
  `POST /agentmemory/session/start`
- startup injection is currently tied to the startup memory prompt path rather
  than the Claude-style hook contract
- `PreToolUse` currently rejects `additionalContext`, which blocks Claude-style
  pre-tool enrichment entirely
- Codex currently supports `additional_context` after tool execution, which is
  useful but is not a substitute for pre-tool enrichment

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
- `memory_recall` is complementary, not a replacement for hook injection

## Explicit Non-Goals

This spec does not require:

- exposing destructive memory tools to the assistant
- broadening pre-tool injection to shell commands
- changing the native memory backend
- replacing the existing `memory_recall` tool contract

## Related Docs

- [`agentmemory-codex-memory-replacement-spec.md`](./agentmemory-codex-memory-replacement-spec.md)
- [`../codex-rs/docs/agentmemory_runtime_surface_spec.md`](../codex-rs/docs/agentmemory_runtime_surface_spec.md)
