# Codex Hooks Implementation Spec

Codex supports Claude-style lifecycle hooks through the `codex-hooks` crate. Hooks are loaded from layered config, evaluated by event-specific runtime code, surfaced through app-server notifications, and shown in the TUI.

This spec describes the intended implementation shape for `~/.codex/hooks.json` compatibility and the current contract that future hook work should preserve.

## Current Status

Codex currently implements the supported command-hook surface for
Claude-compatible `hooks.json` files:

- `hooks.json` and inline TOML hooks load from effective config layers.
- Trusted or managed command hooks run for `SessionStart`, `UserPromptSubmit`,
  `PreToolUse`, `PermissionRequest`, `PostToolUse`, and `Stop`.
- Matching command hooks execute synchronously in configured order.
- `hooks/list` returns runnable hooks plus non-runnable async, prompt, and agent
  hooks with warnings so clients can explain why they are unavailable.
- `suppressOutput` hides hook-authored visible entries without dropping hook
  decisions or model-visible context.
- Unsupported mutation fields fail with explicit hook errors.

Full Claude Code hook parity is not claimed yet. Async execution, prompt hooks,
agent hooks, durable hook ids, and hook-driven input/output rewrites remain
deferred work.

## Goals

- Support a Claude-compatible `hooks.json` file at every config layer folder, including `~/.codex/hooks.json`.
- Preserve Codex config layering: system, user, project, MDM, session flags, managed requirements, and plugin hook sources all participate in discovery.
- Keep unmanaged hooks inert until explicitly trusted by hash.
- Emit hook lifecycle events so UIs can show pending, completed, failed, blocked, and stopped hook runs.
- Keep hook process input and output schema versioned by generated fixtures.

## Non Goals

- No prompt or agent hook execution yet. These handler types may parse, but discovery must skip them with warnings until implemented.
- No async hook execution yet. `async = true` command hooks must be skipped with warnings.
- No hook-driven tool input rewriting yet. Unsupported rewrite fields should fail closed with visible hook errors.
- No v1 app-server API additions. New UI/API surface belongs in app-server v2.

## Configuration

`~/.codex/hooks.json` uses this top-level shape:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "python3 /path/to/hook.py",
            "timeout": 5,
            "statusMessage": "checking command"
          }
        ]
      }
    ]
  }
}
```

The same event shape is also accepted under `[hooks]` in `config.toml`. If both TOML hooks and `hooks.json` are present in one config layer, load both and warn that one representation is preferred.

Supported events:

- `PreToolUse`
- `PermissionRequest`
- `PostToolUse`
- `SessionStart`
- `UserPromptSubmit`
- `Stop`

Events with meaningful matcher dispatch:

- `PreToolUse`
- `PermissionRequest`
- `PostToolUse`
- `SessionStart`

`UserPromptSubmit` and `Stop` ignore matcher fields.

Supported handler fields:

- `type = "command"`: executable hook command, currently the only runnable handler.
- `command`: shell command string. Empty commands are skipped.
- `timeout` / `timeout_sec`: command timeout in seconds, default `600`, minimum `1`.
- `async`: parsed but not supported. `true` skips the hook with a warning.
- `statusMessage`: optional UI text for running hooks.

Parsed but unsupported handler types:

- `type = "prompt"`
- `type = "agent"`

Unsupported handlers are included in discovery/listing metadata with warnings,
but they are never added to the runnable handler set.

## Discovery And Trust

Discovery must return both runnable and non-runnable hooks so UI can explain why a hook is unavailable.

Each discovered command hook receives:

- stable event name
- matcher
- command after source environment substitution
- timeout
- source path and source kind
- plugin id when sourced from a plugin
- display order
- enabled state
- current hash
- trust status
- managed flag

Unmanaged hooks only run when:

- `enabled` is not `false`
- `trustStatus` is `trusted`

Managed hooks run when enabled by their managed source and report `trustStatus = managed`. User state entries for managed hooks must not disable or trust-modify them.

Hook state lives in `hooks.state` in user config. Each key maps to:

```toml
[hooks.state."<hook-key>"]
enabled = true
trusted_hash = "sha256:..."
```

The current key suffix is positional:

```text
<source-identity>:<event_key>:<group_index>:<handler_index>
```

This is user-visible through `hooks/list`, so replacing it with durable ids should be treated as a migration.

## Runtime

Runtime entry points live in `codex-rs/core/src/hook_runtime.rs`. They build event requests, preview matching hooks, emit `HookStarted`, execute hooks, emit `HookCompleted`, and fold event-specific outcomes back into the turn.

Execution rules:

- Use the configured shell program and args from `HooksConfig`.
- Pass event input as JSON on stdin.
- Run commands from the request cwd.
- Execute selected handlers in precedence order.
- Continue through failures unless event-specific parsing returns a block, stop, or permission decision.
- Emit serialization and process failures as completed hook events with `failed` status.

Event behavior:

- `SessionStart`: may add model context or stop the turn.
- `UserPromptSubmit`: may add model context, block the prompt, or stop processing.
- `PreToolUse`: may add model context or block a tool call before execution.
- `PermissionRequest`: may return `allow` or `deny`; any deny wins, otherwise the last allow wins.
- `PostToolUse`: may add model context, add feedback, block transcript-visible result handling, or stop execution.
- `Stop`: may stop, or block stop and inject continuation prompt fragments.

## Hook Process IO

Generated schemas live in `codex-rs/hooks/schema/generated/`. Update them with:

```sh
just write-hooks-schema
```

Inputs include common fields:

- `session_id`
- `turn_id` for turn-scoped events
- `transcript_path`
- `cwd`
- `hook_event_name`
- `model`
- `permission_mode`

Tool events also include:

- `tool_name`
- `tool_input`
- `tool_use_id` for pre/post tool use
- `tool_response` for post tool use

`SessionStart` includes `source` with `startup`, `resume`, or `clear`.

`Stop` includes:

- `stop_hook_active`
- `last_assistant_message`

Outputs use Claude-style universal fields:

- `continue`
- `stopReason`
- `suppressOutput`
- `systemMessage`

`suppressOutput` suppresses hook-authored visible entries such as warning,
feedback, stop, and context output. It does not suppress parse/runtime errors,
and it does not prevent decisions or additional context from affecting the turn.

Event-specific output is nested in `hookSpecificOutput` where supported. Unsupported fields, such as `updatedInput`, `updatedPermissions`, `interrupt`, and `updatedMCPToolOutput`, must fail closed with a visible hook error.

Plain text stdout is accepted as additional context only for `SessionStart` and `UserPromptSubmit`. For other events, JSON-looking invalid output is treated as failure and empty output is success.

Exit code `2` is a compatibility block signal for `PreToolUse`, `PermissionRequest`, `UserPromptSubmit`, and `Stop`; stderr must contain the reason.

## App Server And UI

App-server v2 owns hook API surface:

- `hooks/list` returns discovered hook metadata per cwd.
- `hook/started` and `hook/completed` notifications mirror core hook lifecycle events.

The TUI should render hook lifecycle entries from notifications and retain completed output even when hooks finish before reveal or overlap with tool output. UI-visible changes require insta snapshot coverage.

## Tests

Minimum coverage for hook changes:

- `cargo test -p codex-hooks`
- `cargo test -p codex-core hooks`
- `cargo test -p codex-app-server hooks_list`
- `cargo test -p codex-tui hooks`

When changing hook schemas:

- Run `just write-hooks-schema`.
- Include generated fixture updates.
- Keep `schema_loader` able to parse all generated schemas.

When changing user-visible TUI rendering:

- Run `cargo test -p codex-tui`.
- Review `cargo insta pending-snapshots -p codex-tui`.
- Accept intentional snapshots.

## Implementation Order For New Work

1. Extend config structs in `codex-rs/config/src/hook_config.rs` only when the file shape changes.
2. Extend generated IO schemas in `codex-rs/hooks/src/schema.rs`.
3. Add event parser rules in `codex-rs/hooks/src/engine/output_parser.rs`.
4. Add or update event runtime behavior in `codex-rs/hooks/src/events/`.
5. Wire core turn behavior in `codex-rs/core/src/hook_runtime.rs` or the relevant tool runtime.
6. Expose metadata in `codex-rs/app-server-protocol/src/protocol/v2/hook.rs` only for app-server-visible contract changes.
7. Add app-server, core, hooks, and TUI tests based on the changed surface.

Do not add new hook work to `codex-core` unless it is specifically turn orchestration or tool integration. Hook parsing, discovery, hashing, execution, and schema logic belong in `codex-hooks`.
