# Fork History Squash Plan

Date: 2026-04-20

Current fork tip: `4164ceeaad` (`main`)
Fork point against upstream: `544b4e39e3`
Fork-only commit count: `62`

## Goal

Compress the fork's first-party history into a much smaller stack without
changing the resulting project tree.

The target is a same-tree rewrite:

- current `main` tree is preserved exactly
- first-party history is reduced from `62` commits to a small lane-owned stack
- opaque fixup churn is absorbed into the commits that own the behavior
- the rewritten branch is easier to reason about, review, and replay later

## Non-Goals

- rewrite `main` in place immediately
- combine the entire fork into one giant commit
- mix this cleanup with upstream sync in the same branch
- preserve every intermediate first-party SHA

## Rewrite Model

Do not use an interactive squash directly on `main`.

Instead:

1. start a new execution branch from the fork point `544b4e39e3`
2. restore file groups from current `main`
3. commit those file groups as lane-owned commits
4. verify the final tree matches current `main`

This keeps the rewrite deterministic and lets the resulting branch be compared
against the live fork tip with a simple tree diff.

## Target Commit Shape

Target: `6` commits.

### Lane 1. Fork operations and release posture

Scope:

- `.github/workflows/**`
- `README.md`
- `NOTICE`
- `package.json`
- `docs/fork-intent.md`
- `docs/license.md`
- `docs/public-release-notes.md`

Intent:

- capture private-fork operating posture
- preserve workflow and release-policy divergence from upstream

### Lane 2. Build, perf, and local tooling

Scope:

- `justfile`
- `codex-rs/Cargo.lock`
- `codex-rs/deny.toml`
- `codex-rs/scripts/prune_perf_build_target.sh`
- `codex-rs/tools/**`

Intent:

- isolate build, perf, and tooling drift from product behavior

### Lane 3. Agentmemory backend and runtime core

Scope:

- `codex-rs/core/src/agentmemory/**`
- `codex-rs/core/src/codex/agentmemory_ops.rs`
- `codex-rs/core/src/hook_runtime.rs`
- `codex-rs/core/src/tools/handlers/memory_runtime.rs`
- `codex-rs/core/src/tools/spec.rs`
- `codex-rs/core/src/tools/registry.rs`
- related `codex-rs/core/src/**` support files
- related `codex-rs/core/tests/suite/**` and core test files

Intent:

- own the backend contract, runtime memory behavior, and context plumbing

### Lane 4. Protocol, hook, and app-server surface

Scope:

- `codex-rs/app-server-protocol/**`
- `codex-rs/app-server/**`
- `codex-rs/hooks/**`
- `codex-rs/protocol/**`
- `codex-rs/mcp-server/src/codex_tool_runner.rs`
- `codex-rs/rollout/src/policy.rs`

Intent:

- own on-wire shape, hook event surfaces, and app-server integration

### Lane 5. TUI and replay UX

Scope:

- `codex-rs/tui/**`

Intent:

- own slash commands, history cells, replay rendering, and app-server parity

### Lane 6. Product and design docs

Scope:

- `codex-rs/docs/**`
- `docs/agentmemory-payload-quality.md`
- `docs/agentmemory-payload-quality-followup.md`
- `docs/claude-code-hooks-parity.md`
- `docs/upstream-replay-plan.md`

Intent:

- preserve the design rationale and follow-up specs as a clean docs lane

## Commit Sources To Absorb

This rewrite should absorb:

- opaque fixups like `fix`, `fix buidl`, and import/format cleanup
- post-rebase fallout fixes
- schema refresh commits
- single-file docs narration commits

These should not survive as standalone history in the new branch.

## Verification

The execution branch is valid only if:

- `git diff --stat main...<execution-branch>` is empty
- `git diff --name-only main...<execution-branch>` is empty
- the commit count from the fork point is reduced to the target lane stack

This rewrite is same-tree, so tree equality to `main` is the primary proof.

## Enactment Branch

Create:

`scratch/fork-history-squashed-20260420`

Base:

`544b4e39e3`

## Recommended Next Step

Build the execution branch immediately after committing this plan and stop only
when the new branch matches `main` exactly.
