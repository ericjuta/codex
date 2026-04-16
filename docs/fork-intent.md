# Fork Intent

This repository is a public fork of
[openai/codex](https://github.com/openai/codex).

The intent of this fork is not to rename or replace the upstream project. The
intent is to keep a Codex-compatible fork while adapting the runtime to use
`agentmemory` as the primary long-term memory backend, expand the public hook
surface toward Claude-style lifecycle parity, keep the memory UX coherent
across both TUIs, and trim hosted CI and release machinery to the lanes this
fork actually needs.

This document explains why this fork exists and which parts of the tree changed
to support that goal.

For the release and legal posture for making the repository public and for
shipping public release artifacts, see
[`docs/public-release-notes.md`](./public-release-notes.md).

## Fork Goals

1. Make `agentmemory` the authoritative memory engine instead of keeping two
   competing long-term memory systems.
2. Expose a runtime memory surface that is coherent for both humans and the
   assistant across `tui` and `tui_app_server`.
3. Move Codex hooks closer to Claude Code's documented lifecycle model so
   existing hook setups port with fewer custom patches.
4. Keep the fork operationally legible by retaining only the CI, docs, and
   release machinery that still matters here.
5. Keep provenance, licensing, and release constraints explicit rather than
   burying fork-specific decisions in commit history.

## Change Map

| Area | Intent | Key files |
|---|---|---|
| Memory backend replacement | Make `agentmemory` the primary long-term memory engine and bypass native Codex memory generation when that backend is selected. | [`../codex-rs/core/src/agentmemory/mod.rs`](../codex-rs/core/src/agentmemory/mod.rs), [`../codex-rs/core/src/codex.rs`](../codex-rs/core/src/codex.rs), [`../codex-rs/core/src/memories/phase2.rs`](../codex-rs/core/src/memories/phase2.rs), [`../codex-rs/core/src/memories/tests.rs`](../codex-rs/core/src/memories/tests.rs) |
| Runtime memory surface and UX | Keep memory recall/update/drop visible and coherent across the TUI and app-server-backed TUI mode, and extend the runtime surface toward explicit remember writes, lessons/crystals/insights visibility, and action orchestration backed by the same agentmemory semantics. | [`../codex-rs/docs/agentmemory_runtime_surface_spec.md`](../codex-rs/docs/agentmemory_runtime_surface_spec.md), [`../codex-rs/tui/src/chatwidget.rs`](../codex-rs/tui/src/chatwidget.rs), [`../codex-rs/tui/src/bottom_pane/footer.rs`](../codex-rs/tui/src/bottom_pane/footer.rs), [`../codex-rs/tui/src/app/app_server_adapter.rs`](../codex-rs/tui/src/app/app_server_adapter.rs) |
| Hook parity and lifecycle capture | Expand the public `hooks.json` surface so Claude-oriented hook configurations map onto Codex with fewer surprises and clearer runtime contracts. | [`./claude-code-hooks-parity.md`](./claude-code-hooks-parity.md), [`../codex-rs/hooks/README.md`](../codex-rs/hooks/README.md), [`../codex-rs/hooks/src/engine/config.rs`](../codex-rs/hooks/src/engine/config.rs), [`../codex-rs/hooks/src/engine/discovery.rs`](../codex-rs/hooks/src/engine/discovery.rs), [`../codex-rs/hooks/src/engine/dispatcher.rs`](../codex-rs/hooks/src/engine/dispatcher.rs), [`../codex-rs/hooks/src/schema.rs`](../codex-rs/hooks/src/schema.rs) |
| Fork-scoped CI and release posture | Remove or narrow upstream maintainer workflows that do not add value in this fork, while keeping enough CI and packaging signal for the paths still used here. | [`../codex-rs/docs/github_actions_private_fork_spec.md`](../codex-rs/docs/github_actions_private_fork_spec.md), [`../.github/workflows/rust-ci.yml`](../.github/workflows/rust-ci.yml), [`../.github/workflows/cargo-deny.yml`](../.github/workflows/cargo-deny.yml), [`../.github/workflows/ci.bazelrc`](../.github/workflows/ci.bazelrc), [`../.github/workflows/v8-ci.bazelrc`](../.github/workflows/v8-ci.bazelrc), [`../scripts/stage_npm_packages.py`](../scripts/stage_npm_packages.py) |
| Public source and licensing clarity | Keep the fork publishable as source, preserve third-party notices, and document the remaining constraints around public binary distribution. | [`../README.md`](../README.md), [`./license.md`](./license.md), [`../NOTICE`](../NOTICE), [`../LICENSE`](../LICENSE) |

## What This Fork Is Not

- It is not a claim to authorship over upstream `openai/codex`.
- It is not a separate product with a new license or package identity.
- It is not a promise that upstream release automation or contributor-governance
  workflows remain enabled here.
- It is not a statement that this repository currently provides official public
  Linux binaries.

## Related Docs

- [`README.md`](../README.md)
- [`docs/claude-code-hooks-parity.md`](./claude-code-hooks-parity.md)
- [`codex-rs/docs/agentmemory_runtime_surface_spec.md`](../codex-rs/docs/agentmemory_runtime_surface_spec.md)
- [`codex-rs/docs/github_actions_private_fork_spec.md`](../codex-rs/docs/github_actions_private_fork_spec.md)
- [`docs/public-release-notes.md`](./public-release-notes.md)
- [`docs/license.md`](./license.md)

## Public Release Notes

The release and legal guidance now lives in
[`docs/public-release-notes.md`](./public-release-notes.md).
