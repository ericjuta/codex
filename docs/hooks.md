# Codex Hooks Specification

Codex supports Claude-style lifecycle hooks from `hooks.json` files and inline
`[hooks]` TOML config. Hooks are command handlers that receive a JSON payload on
stdin and may return JSON on stdout to block an action, add context, request a
continuation, or provide user-visible feedback.

## Goals

- Provide a stable `~/.codex/hooks.json` shape that is familiar to Claude Code
  hook users.
- Keep hook execution deterministic, trust-gated, and observable.
- Use one hook engine for user config, project config, managed config, and plugin
  config.
- Preserve current Codex safety semantics: untrusted project config does not run,
  unmanaged hooks must be reviewed, and managed hooks are always controlled by
  the managed source.

## Config Shape

User-level hooks live at `~/.codex/hooks.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "python3 ~/.codex/hooks/pre_tool_use.py",
            "timeout": 30,
            "statusMessage": "checking command"
          }
        ]
      }
    ]
  }
}
```

The same event groups can be configured inline in `config.toml`:

```toml
[hooks]

[[hooks.PreToolUse]]
matcher = "Bash"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "python3 ~/.codex/hooks/pre_tool_use.py"
timeout = 30
statusMessage = "checking command"
```

Supported event keys:

- `PreToolUse`
- `PermissionRequest`
- `PostToolUse`
- `SessionStart`
- `UserPromptSubmit`
- `Stop`

Each event key maps to an array of matcher groups. Each group has:

- `matcher`: optional string. `PreToolUse`, `PermissionRequest`,
  `PostToolUse`, and `SessionStart` use it. `UserPromptSubmit` and `Stop`
  ignore it.
- `hooks`: array of handlers.

Supported handler:

- `type: "command"`
- `command`: shell command string.
- `timeout`: optional seconds, default `600`, minimum `1`.
- `statusMessage`: optional user-visible running label.
- `async`: parsed but currently skipped with a warning when true.

Reserved handler types:

- `type: "prompt"`: parsed but skipped.
- `type: "agent"`: parsed but skipped.

## Discovery And Sources

The engine discovers hooks from each effective config layer:

- `hooks.json` in the config folder for that layer.
- Inline `[hooks]` tables in that layer's `config.toml`.
- Managed hook requirements.
- Enabled plugin hook sources.

If both `hooks.json` and inline TOML hooks are present in the same layer, both
are loaded and a warning is emitted. New docs and examples should prefer one
representation per layer.

Plugin hooks support command substitution for:

- `${PLUGIN_ROOT}`
- `${PLUGIN_DATA}`
- `${CLAUDE_PLUGIN_ROOT}` for compatibility.
- `${CLAUDE_PLUGIN_DATA}` for compatibility.

## Trust And Enablement

Hooks are listed even when they cannot run. A hook runs only when:

- The stable `hooks` feature is enabled.
- Its config layer is loaded for the current `cwd`.
- The hook is enabled.
- The hook is managed, or its current definition hash matches a trusted hash in
  `hooks.state`.

Unmanaged hook state lives in user config under `hooks.state`:

```toml
[hooks.state."/Users/me/.codex/hooks.json:pre_tool_use:0:0"]
enabled = true
trusted_hash = "sha256:..."
```

Trust status values:

- `managed`: controlled by a managed source and not user-editable.
- `untrusted`: first-seen unmanaged hook.
- `trusted`: current hash matches user-approved hash.
- `modified`: current hash differs from the approved hash.

Hook keys are currently positional:

```text
<source identity>:<event_name>:<group_index>:<handler_index>
```

This is acceptable for the current implementation, but a future durable hook id
would make reordering less disruptive.

## Matcher Semantics

An omitted matcher, `""`, or `"*"` matches all supported occurrences.

If a matcher contains only ASCII alphanumeric characters, `_`, or `|`, it is an
exact matcher. Pipe separates exact alternatives:

```text
Bash|Edit|Write
```

Other matcher strings are treated as regular expressions and validated at
discovery time. Invalid regex matchers are skipped with a warning.

Tool hooks match against the canonical hook tool name plus compatibility aliases.
`SessionStart` matches against the session start source. `UserPromptSubmit` and
`Stop` do not use matcher values.

## Execution Model

Command hooks run synchronously in configured order. Codex invokes the user's
shell with the configured command, writes the hook input JSON to stdin, captures
stdout and stderr, and enforces the handler timeout.

Nonzero exits, spawn failures, serialization failures, output parse failures,
and timeouts are recorded as hook failures. Event-specific behavior determines
whether the underlying action continues, blocks, or adds context.

Every run emits:

- `hook/started`
- `hook/completed`

The completed run includes status, timing, source metadata, and output entries
for warnings, stops, feedback, context, and errors.

## Input Payloads

All command inputs include:

- `session_id`
- `transcript_path`
- `cwd`
- `hook_event_name`
- `model`
- `permission_mode`

Turn-scoped events also include `turn_id`.

Event-specific fields:

- `PreToolUse`: `tool_name`, `tool_input`, `tool_use_id`
- `PermissionRequest`: `tool_name`, `tool_input`
- `PostToolUse`: `tool_name`, `tool_input`, `tool_response`, `tool_use_id`
- `SessionStart`: `source`
- `UserPromptSubmit`: `prompt`
- `Stop`: `stop_hook_active`, `last_assistant_message`

The generated JSON schemas under `codex-rs/hooks/schema/generated/` are the
source of truth for the exact payload shape.

## Output Payloads

All command outputs may include universal fields:

- `continue`: defaults to `true`.
- `stopReason`: optional reason to stop processing later hooks.
- `suppressOutput`: omit hook output from user-visible history when true.
- `systemMessage`: optional user-visible/system feedback.

Event-specific outputs:

- `SessionStart`: `hookSpecificOutput.additionalContext`
- `PreToolUse`: legacy `decision: "approve" | "block"` plus `reason`, or
  `hookSpecificOutput.permissionDecision: "allow" | "deny" | "ask"` with
  `permissionDecisionReason` and optional `additionalContext`
- `PermissionRequest`: `hookSpecificOutput.decision.behavior: "allow" | "deny"`
  with optional `message`
- `PostToolUse`: `decision: "block"` plus `reason`, and optional
  `hookSpecificOutput.additionalContext`
- `UserPromptSubmit`: `decision: "block"` plus `reason`, and optional
  `hookSpecificOutput.additionalContext`
- `Stop`: `decision: "block"` plus `reason`

Reserved output fields such as tool input rewrites and MCP output rewrites are
parsed as unsupported today and should fail closed until the runtime implements
the corresponding mutation contract.

Large additional context, feedback, and stop continuation fragments may be
spilled by the runtime before being injected into model-visible context.

`suppressOutput: true` hides hook-authored user-visible entries such as
warnings, feedback, stop text, and context entries. It does not suppress hook
errors, and it does not discard model-visible context or hook decisions.

## Operator Smoke Check

The repo includes a focused smoke check for the currently runnable command-hook
surface:

```shell
cd codex-rs
./scripts/hooks-command-smoke.sh
```

The smoke check builds a temporary Codex home with a generated `hooks.json`,
trusts the discovered unmanaged hooks in that temporary config, and drives a
real test Codex session through all six supported command events.

It proves:

- `SessionStart` injects quiet model-visible context.
- `UserPromptSubmit` injects quiet model-visible context with `suppressOutput`.
- `PreToolUse` blocks a command before it runs.
- `PermissionRequest` allows one command and denies another.
- `PostToolUse` adds model-visible context while hiding hook-authored UI text.
- `Stop` blocks once with a continuation prompt.
- Matching hooks run in configured order.
- No-output successes are quiet but still emit hook lifecycle events.
- Suppressed additional context still reaches the model request.
- Blocked and denied commands return operator-readable feedback.

## Runtime Integration Points

Implementation ownership:

- Config shape: `codex-rs/config/src/hook_config.rs`
- Feature gate: `codex-rs/features/src/lib.rs`
- Discovery, trust, source ordering: `codex-rs/hooks/src/engine/discovery.rs`
- Command execution: `codex-rs/hooks/src/engine/command_runner.rs`
- Input and output schemas: `codex-rs/hooks/src/schema.rs`
- Event behavior: `codex-rs/hooks/src/events/`
- Core turn integration: `codex-rs/core/src/hook_runtime.rs` and
  `codex-rs/core/src/session/turn.rs`
- App-server listing API: `codex-rs/app-server-protocol/src/protocol/v2/hook.rs`
  and `codex-rs/app-server/tests/suite/v2/hooks_list.rs`
- TUI rendering and review UI: `codex-rs/tui/src/chatwidget/hooks.rs`,
  `codex-rs/tui/src/history_cell/hook_cell.rs`, and
  `codex-rs/tui/src/bottom_pane/hooks_browser_view.rs`

## Parity Gaps

Current status: Codex implements Claude-style `hooks.json` command-hook parity
for the supported lifecycle events listed above. Hooks can be discovered from
`~/.codex/hooks.json`, reviewed/trusted, listed through app-server, rendered by
the TUI, and executed in configured order with schema-backed input/output.

Current gaps to close before claiming full Claude Code hook parity:

- Async command hooks are parsed but skipped.
- Prompt and agent hooks are parsed but skipped.
- Hook keys are positional rather than durable ids.
- Tool input/output rewrite fields are reserved but unsupported.
- Permission rewrite and interrupt semantics are reserved but unsupported.
- Public user docs should link the generated schemas and show one complete
  example per event.

## Test Plan

Config and discovery:

- Parse existing `hooks.json` shape.
- Parse inline TOML hooks.
- Merge or warn when both config forms exist in one layer.
- Skip invalid matcher regexes, empty commands, async handlers, prompt handlers,
  and agent handlers with explicit warnings.
- Verify project hooks load only for trusted projects.
- Verify managed hooks ignore user state.
- Verify plugin substitutions and plugin hook warnings.

Runtime:

- Run each event with a command hook and assert stdin payloads.
- Assert blocking behavior for `PreToolUse`, `PostToolUse`,
  `UserPromptSubmit`, and `Stop`.
- Assert permission decisions for `PermissionRequest`.
- Assert `SessionStart` and additional-context injection.
- Assert timeout, spawn failure, nonzero exit, invalid JSON, and unsupported
  output fields.
- Assert stop-hook continuation behavior and `stop_hook_active` recursion guard.

App-server and TUI:

- `hooks/list` returns hooks, warnings, errors, trust state, and source metadata
  for multiple cwd values.
- `config/batchWrite` can disable, re-enable, and trust unmanaged hooks.
- TUI snapshots cover hook start, completion, blocked, failed, context, feedback,
  and review-browser states.

Schema:

- Regenerate fixtures with the hook schema writer when input or output payloads
  change.
- Treat generated schema diffs as part of the API review.
