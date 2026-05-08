# Hooks Implementation Spec

This spec defines the implementation shape for Codex lifecycle hooks configured in
`~/.codex/hooks.json`, with parity-oriented behavior for Claude Code style command
hooks while preserving Codex-specific safety and app-server surfaces.

## Goals

- Load hook definitions from `hooks.json` in every active config layer and from
  inline `[hooks]` TOML without changing the public event contract.
- Execute trusted command hooks for `SessionStart`, `UserPromptSubmit`,
  `PreToolUse`, `PermissionRequest`, `PostToolUse`, and `Stop`.
- Keep hook input and output payloads stable, schema-backed, and safe to expose
  through app-server notifications and transcript history.
- Support review, trust, enablement, diagnostics, plugin hooks, and managed hooks
  through the existing `hooks/list` and `config/batchWrite` flow.
- Leave `prompt`, `agent`, and `async` hook handlers non-runnable until their
  runtime semantics are explicitly designed.

## Current Status

Implemented for the supported command-hook surface:

- `~/.codex/hooks.json` and inline TOML parsing.
- Layered discovery for user, project, managed, session, and plugin hook
  sources.
- Trust-gated execution for unmanaged hooks and always-managed trusted
  execution for managed sources.
- Runnable command hooks for `SessionStart`, `UserPromptSubmit`, `PreToolUse`,
  `PermissionRequest`, `PostToolUse`, and `Stop`.
- Synchronous execution of matching command hooks in configured order.
- `suppressOutput` support that hides hook-authored visible entries while
  preserving hook decisions and model-visible context.
- `hooks/list` visibility for runnable hooks, disabled/untrusted hooks, and
  unsupported async, prompt, and agent handlers.
- Per-active-call network host approval caching so a single tool call does not
  repeatedly request approval for an already allowed host.

Not implemented yet:

- Async command hook execution.
- Prompt or agent hook execution.
- Durable non-positional hook ids.
- Hook-driven tool input/output mutation.
- Permission rewrite or interrupt semantics.
- Additional Claude event names outside the six supported lifecycle events.

## Non-Goals

- Do not add new event names before the six supported lifecycle events are fully
  documented and covered.
- Do not let untrusted user, project, plugin, or session-flag hooks run.
- Do not add network or remote hook handlers.
- Do not let hooks mutate tool input or tool output until a follow-up design
  defines the conflict and audit semantics.
- Do not route new API surface through app-server v1.

## Existing Surfaces

- Config shape lives in `codex-rs/config/src/hook_config.rs`.
  `HooksFile` parses `hooks.json` as `{ "hooks": { ... } }`; `HooksToml` parses
  inline `[hooks]` plus `hooks.state` for enablement and trust.
- Discovery and trust live in `codex-rs/hooks/src/engine/discovery.rs`.
  It loads `hooks.json`, inline TOML, managed requirements, and plugin hook
  sources, then emits runnable `ConfiguredHandler`s only when enabled and trusted
  or managed.
- Runtime dispatch lives in `codex-rs/hooks/src/events/*` and is bridged from
  core through `codex-rs/core/src/hook_runtime.rs`.
- App-server metadata lives in
  `codex-rs/app-server-protocol/src/protocol/v2/hook.rs`,
  `codex-rs/app-server-protocol/src/protocol/v2/plugin.rs`, and
  `hooks/list`.
- TUI review and rendering live in `codex-rs/tui/src/bottom_pane/hooks_browser_view.rs`
  and `codex-rs/tui/src/history_cell/hook_cell.rs`.
- Claude settings migration lives in `codex-rs/external-agent-migration/src/lib.rs`.
  It imports convertible command hooks and intentionally drops unsupported
  handler kinds and unsupported command fields.

## Config Contract

`~/.codex/hooks.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash|apply_patch",
        "hooks": [
          {
            "type": "command",
            "command": "python3 ~/.codex/hooks/policy.py",
            "timeout": 30,
            "statusMessage": "checking policy"
          }
        ]
      }
    ]
  }
}
```

Supported fields:

- Event keys: `PreToolUse`, `PermissionRequest`, `PostToolUse`,
  `SessionStart`, `UserPromptSubmit`, `Stop`.
- Matcher group: optional `matcher` string plus `hooks` array.
- Command handler: `type = "command"`, `command`, optional `timeout`, optional
  `statusMessage`, optional `async`.
- `async = true` is accepted by config parsing but skipped with a warning until
  asynchronous lifecycle behavior is designed.
- `type = "prompt"` and `type = "agent"` are accepted by config parsing but
  skipped with warnings until implemented.
  These unsupported handlers remain visible in discovery/listing metadata, but
  they are not added to the runnable handler set.

Inline TOML under `[hooks]` has the same event and handler model. If both
`hooks.json` and inline TOML define hooks for one config layer, both load today;
the loader warns that a single representation is preferred.

## Discovery, Ordering, and Trust

Discovery order is lowest precedence first by config layer, then plugin hook
sources. Managed requirements are appended before normal config-layer hooks.
Each discovered command handler gets a monotonic `display_order` that is used for
UI order and run IDs.

Runnable conditions:

- Managed hooks are always enabled and trusted.
- Unmanaged hooks default to enabled but not trusted.
- Disabled hooks remain visible in `hooks/list` and do not run.
- Unmanaged hooks run only when `hooks.state.<key>.trusted_hash` matches the
  current normalized hook hash.
- A changed hook becomes `Modified` and must be re-trusted before it can run.

Hash identity must remain based on normalized event, matcher, command, timeout,
and status message so equivalent JSON and TOML definitions converge on the same
trust hash.

Known limitation: hook keys end in positional `event/group/handler` selectors.
A future durable ID should be added before hooks are reordered automatically.

## Event Semantics

`SessionStart`

- Matcher input: session start source, currently `startup`, `resume`, or `clear`.
- Input includes `session_id`, `transcript_path`, `cwd`, `hook_event_name`,
  `model`, `permission_mode`, and `source`.
- Plain stdout is accepted as additional context. JSON stdout can add
  `hookSpecificOutput.additionalContext`.
- `continue: false` stops session startup and can emit `stopReason`.

`UserPromptSubmit`

- Matchers are ignored.
- Input includes `session_id`, `turn_id`, `transcript_path`, `cwd`,
  `hook_event_name`, `model`, `permission_mode`, and `prompt`.
- Plain stdout is accepted as additional context. JSON stdout can add
  additional context or `decision: "block"` with a reason.
- A block prevents the submitted prompt from entering the turn.

`PreToolUse`

- Matcher input is the canonical tool name plus compatibility aliases.
- Input includes `session_id`, `turn_id`, `transcript_path`, `cwd`,
  `hook_event_name`, `model`, `permission_mode`, `tool_name`, `tool_input`, and
  `tool_use_id`.
- A hook can block with legacy `decision: "block"` plus `reason`, or with
  `hookSpecificOutput.permissionDecision: "deny"` plus
  `permissionDecisionReason`.
- Additional context is forwarded to the model.
- `updatedInput` is currently rejected as unsupported.

`PermissionRequest`

- Matcher input is the canonical tool name plus compatibility aliases.
- Input includes `session_id`, `turn_id`, `transcript_path`, `cwd`,
  `hook_event_name`, `model`, `permission_mode`, `tool_name`, and `tool_input`.
- Output can allow or deny through
  `hookSpecificOutput.decision.behavior = "allow" | "deny"`.
- Any deny wins; otherwise the last allow wins; otherwise normal approval flow
  continues.
- `updatedInput`, `updatedPermissions`, and `interrupt` are currently rejected.

`PostToolUse`

- Matcher input is the canonical tool name plus compatibility aliases.
- Input includes all `PreToolUse` fields plus `tool_response`.
- Output can add model context, block the result with feedback, or stop the turn
  with `continue: false`.
- `updatedMCPToolOutput` is currently rejected as unsupported.

`Stop`

- Matchers are ignored.
- Input includes `session_id`, `turn_id`, `transcript_path`, `cwd`,
  `hook_event_name`, `model`, `permission_mode`, `stop_hook_active`, and
  `last_assistant_message`.
- `decision: "block"` plus `reason` creates a continuation prompt and causes the
  model to continue the turn.
- `stop_hook_active` lets hooks avoid infinite continuation loops.

## Execution Semantics

- Hooks run through the configured command shell, defaulting to `$SHELL -lc` on
  Unix and `%COMSPEC% /C` on Windows.
- Command stdin is the event-specific JSON payload.
- Command stdout is parsed as event-specific JSON unless the event explicitly
  allows plain text context.
- Command stderr is used as the blocking reason for exit code `2` on events that
  support that Claude-style convention.
- Commands run with `kill_on_drop` and a per-handler timeout. Missing timeout
  defaults to 600 seconds and is clamped to at least 1 second.
- Matching command handlers execute synchronously in configured order.
- Hook start and completion events are emitted around runtime execution so the
  TUI and app-server clients can render status without waiting for completion.

## Output and Transcript Policy

- Successful hooks with no output are quiet and should not leave transcript
  artifacts.
- `suppressOutput: true` suppresses hook-authored visible warning, feedback,
  stop, and context entries. It does not suppress explicit hook errors, and it
  does not discard model-visible context or hook decisions.
- Failed, blocked, stopped, warning, feedback, context, and error entries are
  visible through `HookCompleted` notifications and TUI history cells.
- Large additional context, feedback, and continuation prompt text can spill to a
  file through the hook output spiller; the model receives a compact pointer.
- Hook-authored text enters the transcript as structured hook prompt/context
  fragments where available, not as raw assistant text.

## Implementation Work Items

1. Documentation

   - Add user-facing hook docs that describe `hooks.json`, inline TOML,
     supported events, trust, and review workflow.
   - Link generated schemas in `codex-rs/hooks/schema/generated`.
   - Document unsupported fields as accepted-but-skipped or rejected-at-runtime.

2. Schema and API polish

   - Ensure `just write-config-schema` captures any config changes.
   - Keep command input/output schema fixtures updated with
     `codex-rs/hooks/src/bin/write_hooks_schema_fixtures.rs`.
   - Keep app-server v2 `HookMetadata` aligned with every handler type that
     becomes runnable.

3. Trust and UX hardening

   - Preserve the current trust gate for all unmanaged sources.
   - Keep `hooks/list` returning disabled and untrusted hooks.
   - Keep TUI review actions writing `hooks.state` only, not mutating hook
     definitions.

4. Runtime hardening

   - Add tests that prove command failures do not crash turns.
   - Add tests for multiple matching handlers where aggregation order matters.
   - Add regression tests for JSON-vs-TOML equivalent trust hashes.
   - Add tests for invalid JSON stdout for every event.

5. Deferred parity

   - Design `async` hook handlers before enabling them. Required decisions:
     lifecycle ownership, cancellation, transcript visibility, ordering, and
     shutdown semantics.
   - Design `prompt` hooks before enabling them. Required decisions: who receives
     the prompt, how model output is bounded, and whether output can block or
     inject context.
   - Design `agent` hooks before enabling them. Required decisions: subagent
     permissions, thread/source attribution, budget limits, and result merging.
   - Design input/output mutation before enabling `updatedInput`,
     `updatedPermissions`, or `updatedMCPToolOutput`.

## Test Plan

Focused local checks after doc-only edits:

- No Rust tests required when only this document changes.

Required checks for runtime/config changes:

- `cd codex-rs && just fmt`
- `cd codex-rs && cargo test -p codex-hooks`
- `cd codex-rs && cargo test -p codex-core hooks`
- `cd codex-rs && cargo test -p codex-app-server hooks_list`
- `cd codex-rs && cargo test -p codex-app-server-protocol`
- If `ConfigToml` or nested config types change:
  `cd codex-rs && just write-config-schema`
- If app-server protocol shapes change:
  `cd codex-rs && just write-app-server-schema`

## Acceptance Criteria

- A user can place command hooks in `~/.codex/hooks.json`, review/trust them,
  and see them run for the supported lifecycle events.
- App-server clients can list hooks, inspect trust status, toggle enablement, and
  trust modified hooks without reading private config files directly.
- Runtime hook events are visible but quiet for no-output successes.
- Unsupported handler types and mutation fields fail predictably with warnings or
  explicit hook errors, not silent partial execution.
- Existing plugin and external-agent migration paths continue to import only
  supported command hooks.
