# Hashline Enablement Flags Spec

## Purpose

Define the proposed runtime configuration contract for:

    [features]
    hashline = true
    hashline_only = true

This spec is intentionally narrower than the broader
[Hashline Tool Integration Spec](hashline_tool_integration_spec.md). That
document defines the native tool architecture and replacement criteria. This
document defines the exact enablement semantics for the two feature booleans
the implementation should add.

Live proof used for this spec:

| Surface | Live state | Implication |
| --- | --- | --- |
| Existing integration spec | `codex-rs/docs/hashline_tool_integration_spec.md` exists in this branch | This spec back-links to it and specializes its gate/config language. |
| Config source type | `codex-rs/features/src/lib.rs` owns the feature registry consumed by `ConfigToml.features` | Add proposed feature keys there if this spec is implemented. |
| Effective config assembly | `codex-rs/core/src/config/mod.rs` builds `Config` from `ConfigToml` and features | Resolve and validate the two booleans there. |
| Tool visibility planning | `codex-rs/core/src/tools/spec_plan.rs` chooses model-visible tools | Enforce additive vs Hashline-only tool visibility there. |
| Existing edit tool | `codex-rs/apply-patch` plus `core/src/tools/handlers/apply_patch.rs` | Keep runtime compatibility unless `hashline_only` explicitly hides model visibility. |

## Config Contract

Add two config keys under `[features]`:

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `hashline` | bool | `false` | Enable native Hashline tool registration alongside existing edit tools. |
| `hashline_only` | bool | `false` | When `[features].hashline = true`, make Hashline the model-visible edit lane and hide direct `apply_patch` from the model where compatibility allows. |

The requested exclusive-mode config is:

    [features]
    hashline = true
    hashline_only = true

Use `[features]` because these are experimental edit-surface gates and should
share the same validation, schema, and CLI enablement paths as other runtime
feature flags.

## Semantics Matrix

| `hashline` | `hashline_only` | Effective mode | Tool visibility |
| --- | --- | --- | --- |
| `false` | `false` | Legacy edit mode | Existing tools only. |
| `true` | `false` | Additive Hashline mode | Hashline tools and existing edit tools are model-visible. |
| `true` | `true` | Hashline-only edit mode | Hashline tools are model-visible; direct `apply_patch` is hidden from the model but retained for compatibility dispatch where required. |
| `false` | `true` | Invalid config | Reject with `features.hashline_only requires features.hashline = true`. |

`hashline_only` must not silently enable Hashline by itself. Requiring both
booleans keeps user intent explicit and avoids a surprising replacement of the
editing surface.

## Effective Runtime Model

Add a small resolved config shape, owned by `codex-core`, rather than checking
raw `ConfigToml` booleans throughout tool planning:

| Field | Type | Meaning |
| --- | --- | --- |
| `hashline.enabled` | bool | Native Hashline handlers may be registered. |
| `hashline.only` | bool | Existing direct edit tools should be hidden from model-visible specs when Hashline has parity for that operation. |

Suggested Rust shape:

    pub struct HashlineConfig {
        pub enabled: bool,
        pub only: bool,
    }

The final names can follow nearby config style, but call sites should avoid
passing raw booleans positionally. Use named fields or an enum such as
`HashlineToolMode`.

## Tool Visibility Rules

When `[features].hashline = true`:

1. Register `hashline.read`, `hashline.patch`, and `hashline.find_block`.
2. Keep existing `apply_patch` behavior unchanged.
3. Add concise model guidance that Hashline is preferred for line-anchored
   edits, while `apply_patch` remains available for broader compatibility.

When `[features].hashline = true` and `[features].hashline_only = true`:

1. Register `hashline.read`, `hashline.patch`, and `hashline.find_block`.
2. Hide `apply_patch` from model-visible specs once Hashline patch parity exists
   for the selected model/tool mode.
3. Keep `ApplyPatchHandler` dispatch-only if needed for:
   - shell interception of legacy `apply_patch` invocations;
   - replay/resume compatibility with old rollouts;
   - code paths that already produced an `apply_patch` call before a config
     reload;
   - internal tests or migration tooling that depend on the standalone
     `apply_patch` command contract.
4. If Hashline lacks parity for an operation, prefer a clear model instruction
   over silent fallback. The model-visible editing contract should not claim
   Hashline-only while still advertising direct `apply_patch`.

## Config Loading and Validation

Implementation touchpoints:

| Step | File | Requirement |
| --- | --- | --- |
| Register feature keys | `codex-rs/features/src/lib.rs` | Add `Feature::Hashline` and `Feature::HashlineOnly` with keys `hashline` and `hashline_only`. |
| Resolve effective config | `codex-rs/core/src/config/mod.rs` | Read resolved `Features`, validate impossible combinations, and store a resolved config field on `Config`. |
| Schema update | `codex-rs/core/config.schema.json` | Run `just write-config-schema` after changing `ConfigToml`. |
| Tool planning | `codex-rs/core/src/tools/spec_plan.rs` | Use resolved config to add Hashline tools and adjust `apply_patch` exposure. |
| Tests | `codex-rs/core/src/config/config_tests.rs`, `core/src/tools/spec_plan_tests.rs` | Cover parsing, validation, and model-visible tool sets. |

Validation rules:

1. `[features].hashline_only = true` with `[features].hashline = false` is an error.
2. `[features].hashline_only = true` before native Hashline patch support exists is an
   error, not a partial no-op.
3. `[features].hashline = true` without a turn environment should not register file tools,
   matching existing file-tool behavior.
4. Unknown nested forms such as `[hashline] enabled = true` are not part of this
   spec.

Preferred error text:

    features.hashline_only requires features.hashline = true

## Tool Planning Detail

The existing planner registers `ApplyPatchHandler` in `add_core_utility_tools`
when an environment exists and the selected model supports an apply-patch tool
type. Hashline mode should slot into the same edit-tool decision area, ideally
by extracting a small helper:

    add_file_edit_tools(context, planned_tools)

The helper should decide:

| Mode | Hashline handlers | ApplyPatch handler |
| --- | --- | --- |
| Legacy | not registered | current exposure |
| Additive | direct/model-visible | current exposure |
| Hashline-only | direct/model-visible | hidden/dispatch-only when compatibility requires it |

Avoid scattering `hashline_only` checks through unrelated tool sources.

## Code Mode Behavior

Hashline mode must work in direct model tools and code-mode nested tools.

Requirements:

1. In normal direct tool mode, expose the `hashline` namespace according to the
   matrix above.
2. In mixed code mode, include Hashline in code-mode nested tool definitions
   when normal file-edit tools would be included.
3. In code-mode-only, `[features].hashline_only = true` should mean the code-mode execute
   prompt advertises Hashline as the edit surface, not `apply_patch`.
4. If code-mode cannot represent a Hashline tool shape yet,
   `[features].hashline_only = true` must fail config/tool-planning validation
   rather than silently showing `apply_patch`.

## Migration Behavior

`hashline_only` is a model-visible replacement mode, not a deletion of old
runtime behavior.

Keep these compatibility paths until there is explicit evidence they are safe to
remove:

1. Standalone `apply_patch` self-invocation.
2. Shell interception that recognizes legacy `apply_patch` commands.
3. Rollout resume for turns that already contain `apply_patch` tool calls.
4. Hook payload compatibility for existing apply-patch hooks.

When `[features].hashline_only = true`, new model-visible instructions should say that
Hashline is the edit path. They should not say `apply_patch` was removed.

## Tests

Config tests:

| Test | Expected result |
| --- | --- |
| Empty config | `HashlineConfig { enabled: false, only: false }`. |
| `[features] hashline = true` | `enabled = true`, `only = false`. |
| `[features] hashline = true`, `hashline_only = true` | `enabled = true`, `only = true`. |
| `[features] hashline_only = true` alone | Config load error with `features.hashline_only requires features.hashline = true`. |
| Schema generation | `hashline` and `hashline_only` appear as `[features]` booleans. |

Tool-planning tests:

| Mode | Expected model-visible tools |
| --- | --- |
| Legacy | No Hashline tools; current `apply_patch` behavior. |
| Additive | Hashline tools plus current `apply_patch` behavior. |
| Hashline-only | Hashline tools visible; `apply_patch` hidden or dispatch-only. |
| No environment | No file mutation/read tools. |
| Code mode | Code-mode nested tool list follows the same visibility rule. |

Integration tests:

1. `[features] hashline = true` lets the model call Hashline read and patch.
2. `[features] hashline_only = true` prevents a new model turn from seeing direct
   `apply_patch`.
3. Resume/replay can still handle an old `apply_patch` call when
   `[features] hashline_only = true`.
4. Hooks and approvals still fire for file mutations.

## Documentation Updates

When implemented, update:

| File | Change |
| --- | --- |
| `docs/config.md` or the current config docs surface | Document `[features].hashline` and `[features].hashline_only`. |
| `docs/example-config.md` | Add a small commented example only after the feature is ready for users. |
| `codex-rs/docs/hashline_tool_integration_spec.md` | Keep the backlink and update integration status. |

## Non-Goals

1. Do not implement Hashline by auto-registering the external MCP server.
2. Do not remove `apply_patch` runtime compatibility in the same change that
   adds `hashline_only`.
3. Do not treat `[features].hashline_only = true` as a no-op when Hashline parity is
   missing.
4. Do not add the config fields without updating the generated config schema.
