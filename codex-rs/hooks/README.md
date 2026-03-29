# Codex Hooks

Codex supports lifecycle hooks configured through `hooks.json` files discovered
from the active config layers.

For repo-local usage, put the config in:

```text
<repo>/.codex/hooks.json
```

and store any helper scripts beside it, for example:

```text
<repo>/.codex/hooks/allium-check.mjs
```

This is the Codex equivalent of Claude-style repo hooks such as
`.claude/hooks/...`.

## Scope And Discovery

Hook configs are discovered from config folders in precedence order.

Common locations:

- User/global: `~/.codex/hooks.json`
- Project/repo: `<repo>/.codex/hooks.json`
- System config folder: `hooks.json` beside the system config layer

Project-level hooks are supported because project config layers resolve to the
repo `.codex/` directory.

## File Format

`hooks.json` uses this shape:

```json
{
  "hooks": {
    "EventName": [
      {
        "matcher": "optional-regex-or-*",
        "hooks": [
          {
            "type": "command",
            "command": "node ./.codex/hooks/example.mjs",
            "timeout": 60,
            "statusMessage": "Running repo hook"
          }
        ]
      }
    ]
  }
}
```

## Supported Events

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

## Supported Hook Types

Currently supported:

- `command`

Recognized but currently skipped:

- `prompt`
- `agent`

Also note:

- `async: true` is parsed but not supported yet, so async hooks are skipped.
- On Windows, `hooks.json` lifecycle hooks are currently disabled.

## Matcher Behavior

`matcher` is mainly useful for event families that naturally carry a target,
such as tool names or session-start source names.

Examples:

- `PreToolUse` / `PostToolUse` / `PostToolUseFailure`: match against the tool
  name, such as `Bash`, `Edit`, or `Write`
- `SessionStart`: match against startup source names
- `UserPromptSubmit` and `Stop`: matcher is ignored

`*` means match-all.

## Repo-Local Example

Run an Allium validation script after edit-like tools:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write|apply_patch",
        "hooks": [
          {
            "type": "command",
            "command": "node ./.codex/hooks/allium-check.mjs",
            "timeout": 60,
            "statusMessage": "Running Allium checks"
          }
        ]
      }
    ]
  }
}
```

## Another Example

Run a lightweight repo bootstrap check at session start:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "startup|resume",
        "hooks": [
          {
            "type": "command",
            "command": "./.codex/hooks/session-start-check.sh",
            "timeout": 30,
            "statusMessage": "Checking repo environment"
          }
        ]
      }
    ]
  }
}
```

## Notes

- Hook discovery is config-layer based, not `AGENTS.md` based.
- `AGENTS.md` is for instructions; `hooks.json` is for executable lifecycle
  hooks.
- If multiple config layers define hooks, lower-precedence layers are loaded
  first and higher-precedence layers are appended later.
