# Claude Code hooks parity

This document captures the current Codex hooks surface and the remaining
feature-parity gap versus Claude Code's documented hooks system. It is intended
to be the canonical planning doc for expanding `codex_hooks`.

## Goal

Bring Codex's public `hooks.json` lifecycle hooks close enough to Claude
Code's model that Claude-oriented hook setups can be ported with predictable,
documented edits rather than custom runtime patches.

This does not require byte-for-byte compatibility in one step. It does require:

- matching the major public event categories users expect,
- supporting the handler types those configurations rely on,
- honoring documented decision-control fields when they are accepted by schema,
- documenting any intentional deltas that remain.

## Read order

If you are implementing against this doc, read the current source in this order:

1. `docs/claude-code-hooks-parity.md`
2. `codex-rs/hooks/src/engine/config.rs`
3. `codex-rs/hooks/src/engine/discovery.rs`
4. `codex-rs/hooks/src/schema.rs`
5. `codex-rs/hooks/src/engine/output_parser.rs`
6. `codex-rs/core/src/hook_runtime.rs`
7. `codex-rs/core/src/codex.rs`
8. `codex-rs/core/src/tools/registry.rs`

This order moves from public contract to discovery, then schema, then parser,
then runtime wiring, then legacy behavior.

## Current source snapshot

This doc is based on the current implementation shape in this checkout:

- public `hooks.json` event groups are defined in
  `codex-rs/hooks/src/engine/config.rs`,
- handler discovery and unsupported-handler warnings live in
  `codex-rs/hooks/src/engine/discovery.rs`,
- public wire schema lives in `codex-rs/hooks/src/schema.rs`,
- output acceptance and rejection behavior lives in
  `codex-rs/hooks/src/engine/output_parser.rs`,
- runtime dispatch for start, prompt-submit, pre-tool, and post-tool hooks
  lives in `codex-rs/core/src/hook_runtime.rs`,
- `Stop` hook wiring lives in `codex-rs/core/src/codex.rs`,
- deprecated legacy `AfterToolUse` dispatch still exists in
  `codex-rs/core/src/tools/registry.rs`,
- no repository-local `hooks.json` files are checked into this tree today.

## Current Codex surface

Today Codex exposes five public `hooks.json` event groups:

- `PreToolUse`
- `PostToolUse`
- `SessionStart`
- `UserPromptSubmit`
- `Stop`

The current engine only executes synchronous command handlers. `prompt`,
`agent`, and `async` configurations are parsed but skipped with warnings.

The current runtime also has narrower execution coverage than Claude Code:

- `PreToolUse` and `PostToolUse` are currently wired through the shell path,
  with runtime requests using `tool_name: "Bash"`.
- `UserPromptSubmit` and `Stop` ignore matchers.
- some wire fields are present in schema but are rejected by the output parser
  as unsupported.

Legacy internal paths still exist for notification-style hooks
(`AfterAgent` / deprecated `AfterToolUse`), but they are not part of the
public `hooks.json` contract.

## Claude Code parity gap

Claude Code's current hooks reference documents a larger event surface and more
handler modes than Codex currently supports.

### Missing event coverage

Codex does not yet expose public `hooks.json` support for these documented
Claude Code event families:

- `InstructionsLoaded`
- `PermissionRequest`
- `PostToolUseFailure`
- `Notification`
- `SubagentStart`
- `SubagentStop`
- `StopFailure`
- `TeammateIdle`
- `TaskCompleted`
- `ConfigChange`
- `CwdChanged`
- `FileChanged`
- `WorktreeCreate`
- `WorktreeRemove`
- `PreCompact`
- `PostCompact`
- `SessionEnd`
- `Elicitation`
- `ElicitationResult`

### Missing handler coverage

Codex does not yet support these Claude Code hook handler categories in the
public engine:

- async command hooks,
- HTTP hooks,
- prompt hooks,
- agent hooks.

### Partial decision-control coverage

Codex schema already models some advanced fields, but runtime support is still
partial:

- `PreToolUse.updatedInput` is rejected.
- `PreToolUse.additionalContext` is rejected.
- `PreToolUse.permissionDecision: allow` is rejected.
- `PreToolUse.permissionDecision: ask` is rejected.
- `PostToolUse.updatedMCPToolOutput` is rejected.
- `suppressOutput` is rejected for `PreToolUse` and `PostToolUse`.
- `stopReason` and `continue: false` are rejected for `PreToolUse`.

This creates a confusing state where the schema shape suggests broader support
than the runtime actually honors.

### Tool and matcher parity gaps

- `PreToolUse` and `PostToolUse` should evolve from shell-centric wiring to
  a consistent tool-event contract across relevant tool classes.
- matcher support should be explicit and consistent across all events that
  Claude users expect to filter.
- MCP-aware hook behavior should be designed as first-class runtime behavior,
  not as a schema placeholder.

## Non-goals

- Reproducing Claude Code internals exactly where Codex architecture differs.
- Preserving every existing partial or deprecated behavior forever.
- Adding public hook types without app-server, TUI, and docs visibility for the
  resulting runs.

## Design principles

- **Public contract first**: do not expose schema fields that the runtime will
  immediately reject unless they are clearly marked unsupported.
- **Event completeness over aliases**: add real lifecycle events before adding
  compatibility shims.
- **One event, one payload contract**: every public event needs stable input and
  output schema fixtures, runtime execution, and surfaced hook-run reporting.
- **Fail-open unless explicitly blocking**: invalid hook output should not cause
  surprising hard failures outside events whose contract is intentionally
  blocking.
- **No hidden UI drift**: hook additions must be visible in the TUI and
  app-server surfaces anywhere hook runs are rendered today.

## Do not do

- Do not add a new public event without input schema, runtime dispatch,
  hook-run reporting, and docs in the same lane.
- Do not keep wire fields in public schema as if they are live when the parser
  still rejects them.
- Do not use deprecated `AfterAgent` or legacy `AfterToolUse` internals as
  the long-term public parity path.
- Do not widen event coverage while leaving handler type and execution mode
  reporting misleading in run summaries.
- Do not make hook support TUI-only; app-server and protocol surfaces must stay
  aligned.

## Implementation plan

### Branch and PR order

Prefer this implementation order:

1. contract cleanup for the existing five events,
2. runtime event expansion on the command-hook engine,
3. handler-type and execution-mode expansion,
4. advanced decision-control support,
5. pre/post tool-class parity work,
6. final doc consolidation and examples.

Do not mix all six into one change. Keep each lane reviewable.

### Phase 1: make the current public surface coherent

Goal: remove misleading partial support inside the existing five events.

Required work:

- align schema and parser behavior for the five existing events,
- either implement or remove unsupported schema fields that are already emitted
  in fixtures,
- document matcher behavior explicitly,
- document current shell-centric tool coverage explicitly,
- add a dedicated user-facing reference doc for `hooks.json` behavior if the
  main docs site still only mentions legacy notification hooks.

Acceptance:

- no schema field is silently accepted but runtime-rejected without explicit
  documentation,
- the docs explain exactly which event fields and decisions are live,
- existing five-event behavior is covered by tests and schema fixtures.

### Phase 2: expand event coverage on the existing command-hook engine

Goal: add missing lifecycle events before broadening handler types.

Priority order:

1. `PermissionRequest`
2. `Notification`
3. `SubagentStart` and `SubagentStop`
4. `PostToolUseFailure` and `StopFailure`
5. `SessionEnd`
6. `ConfigChange`, `CwdChanged`, and `FileChanged`
7. `PreCompact` and `PostCompact`
8. `TaskCompleted` and `TeammateIdle`
9. `InstructionsLoaded`
10. `WorktreeCreate` and `WorktreeRemove`
11. `Elicitation` and `ElicitationResult`

Acceptance:

- each event has an input schema fixture,
- each event has runtime dispatch wiring,
- each event emits `HookStarted` and `HookCompleted` consistently,
- each event has an explicit matcher story,
- docs list the event as supported.

### Phase 3: broaden handler types

Goal: match the main Claude Code hook execution modes.

Required work:

- implement async command hooks,
- add HTTP hook handlers,
- add prompt hook handlers,
- add agent hook handlers,
- surface handler type and execution mode accurately in run summaries.

Acceptance:

- discovery no longer skips supported handler types with warnings,
- `HookRunSummary` reports real handler type and execution mode,
- command, HTTP, prompt, and agent handlers have stable input/output contracts,
- async execution semantics are documented, especially ordering and failure
  behavior.

### Phase 4: close decision-control parity gaps

Goal: implement or explicitly drop advanced output fields.

Required work:

- decide whether `PreToolUse.updatedInput` will be supported in Codex,
- decide whether `PreToolUse.permissionDecision: ask` maps to an approval
  prompt, a model-visible continuation, or remains unsupported,
- implement `additionalContext` anywhere the contract claims it exists,
- decide whether `PostToolUse.updatedMCPToolOutput` is part of the public
  runtime contract,
- review event-specific `continue`, `stopReason`, and `suppressOutput`
  semantics for consistency.

Acceptance:

- advanced hook output fields are either implemented end-to-end or removed from
  public schema,
- runtime behavior matches docs and tests,
- no event-specific decision-control behavior relies on undocumented parser
  special cases.

### Phase 5: tool-class parity for pre/post tool hooks

Goal: make tool hooks genuinely tool-aware rather than shell-specific.

Required work:

- define which Codex tool classes participate in `PreToolUse` and
  `PostToolUse`,
- expose stable tool identifiers and input payloads for those classes,
- define MCP-tool matcher behavior explicitly,
- preserve backward compatibility for existing Bash-oriented hooks where
  feasible.

Acceptance:

- users can target more than the shell path with pre/post tool hooks,
- tool names and payloads are documented and stable,
- MCP tool behavior is implemented rather than placeholder-only.

## Required cross-cutting work

- update docs under `docs/` when public behavior changes,
- keep generated schema fixtures in sync,
- extend TUI and app-server visibility for new hook events when needed,
- add focused tests for parser behavior, discovery behavior, and runtime
  dispatch,
- decide whether legacy notification hooks remain supported long term or are
  explicitly deprecated in docs.

## Acceptance gates for any implementation PR

Every parity PR should satisfy all of these before merge:

- docs updated for the newly supported behavior,
- generated hook schema fixtures updated if the public schema changed,
- focused tests added or updated for discovery, parser, and runtime behavior,
- hook run summaries still render correctly in TUI and app-server surfaces,
- unsupported behavior is either removed from schema or clearly documented as
  unsupported.

## Open decisions

- Should Codex aim for Claude-compatible field names and semantics wherever
  possible, or only for event-name parity?
- Should prompt and agent hooks be first-class in the initial public contract,
  or stay experimental behind feature flags after implementation?
- Should unsupported advanced fields be removed now to reduce confusion, or kept
  in schema as forward-compatibility placeholders?
- Which events should be thread-scoped versus turn-scoped in app-server and TUI
  reporting?

## Recommended first implementation slice

If this work is started incrementally, the highest-leverage first slice is:

1. publish a real user-facing hooks reference for Codex,
2. make the existing five events internally coherent,
3. add `PermissionRequest`, `Notification`, `SubagentStart`,
   `SubagentStop`, and `SessionEnd`,
4. then add async and HTTP handler support.

That sequence closes the largest user-visible parity gaps without mixing event
expansion, execution-model expansion, and advanced mutation semantics into one
hard-to-review change.
