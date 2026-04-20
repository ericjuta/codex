# Upstream Rebase Plan Spec

Date: 2026-04-20

Current fork `main`: `4164ceeaad` (`origin/main`, `main`)
Target upstream base: `e53e6bc48f` (`openai/codex` `upstream/main`)
Current fork point: `544b4e39e3`

## Goal

Move the fork onto current upstream while preserving the intentional fork value:

- agentmemory as the primary memory backend
- expanded runtime memory surfaces
- mission, handoff, review, and retrieval-trace memory UX
- fork-specific docs and private-fork operating guidance
- local-only build and workflow adjustments that still make sense on top of
  upstream

## Strategy Decision

Do not run a blind `git rebase upstream/main` on the current fork `main`.

Use a scratch rebase plus semantic replay:

1. Create a scratch branch from `upstream/main`.
2. Replay fork work by lane instead of by raw commit order.
3. Use `cherry-pick -x` only for clean self-contained commits.
4. Use patch transport or semantic backports when file layout or APIs diverged.
5. Squash fallout and fixup commits into the lane that owns the behavior.
6. Validate lane-by-lane before merging the scratch result back into the fork.

This is operationally a rebase plan, but the safe implementation mode is
rebase-plus-replay rather than literal history preservation.

## Current Graph Facts

As of 2026-04-20:

- fork `main` is `62` commits ahead of `upstream/main`
- fork `main` is `187` commits behind `upstream/main`
- fork delta since the fork point touches `161` files
- upstream delta since the fork point touches `825` files
- the overlapping change surface is `91` files

Highest-overlap areas:

- `codex-rs/app-server-protocol/schema/json`
- `codex-rs/core/src/tools`
- `codex-rs/hooks/src/events`
- `codex-rs/hooks/src/engine`
- `codex-rs/tui/src/chatwidget*`
- `codex-rs/tui/src/bottom_pane/*`
- `codex-rs/core/src/hook_runtime.rs`
- `codex-rs/core/src/codex.rs`

Implication: the fork is too far from upstream for a low-risk mechanical rebase,
but still close enough for a structured replay.

## Non-Goals

- preserve every existing fork commit SHA
- carry forward formatting-only or import-order-only churn
- replay opaque fixup commits without first mapping them to a concrete lane
- reintroduce stale replay artifacts or ad hoc patch files
- force fork-only workflow deletions onto upstream if the current workflow graph
  has materially changed

## Proposed Execution Model

### 1. Create the scratch branch

Start from current upstream:

`git switch -c scratch/rebase-20260420-upstream-main upstream/main`

### 2. Establish baseline validation

Before replaying fork code:

- confirm the upstream branch builds in the touched crates
- record any pre-existing upstream failures that are unrelated to the fork

### 3. Replay the fork by lane

Do not replay commit-by-commit in chronological order.

Instead, rebuild the fork in behavior-owned slices:

1. base agentmemory backend and lifecycle lane
2. config-first hook parity and runtime surface lane
3. mission, handoff, review, and retrieval-trace lane
4. TUI and app-server parity lane
5. docs and fork-operations lane
6. optional perf/build lane

### 4. Validate after each lane

After each lane:

- run the smallest relevant crate tests
- inspect generated schema changes
- review the TUI/app-server diffs before proceeding

### 5. Merge the scratch branch back into the fork

Once the scratch branch is stable:

- open a PR from the scratch branch into the fork `main`
- keep the old branch history for archaeology, not for further replay

## Lane Order

### Lane 1. Base agentmemory backend and lifecycle

Replay first:

- `792bc7d7ba` `feat: replay agentmemory backend surface`
- `0e18270cd9` `feat: replay structured memory operation events`
- `e5350aa8b0` `fix: wire agentmemory session lifecycle`
- `c911e321a1` `fix(agentmemory): persist memory replay and capture lifecycle hooks`
- `badb1e5b7e` `fix(agentmemory): summarize sessions and report empty updates`

Expected result:

- agentmemory is the active backend end-to-end
- session lifecycle and replay semantics are coherent
- structured memory operation events exist at protocol/runtime boundaries

Target cleanup shape: `2` to `4` commits.

### Lane 2. Config-first hook parity and runtime surface

Replay after the backend is stable:

- `730e7294c9` `feat: implement config-first agentmemory hook parity`
- `f57840085b` `feat(agentmemory): expand runtime memory surfaces`
- `6fa2a25cd2` `feat(agentmemory): tighten native observe payload contract`
- `306f14fcab` `Optimize agentmemory context planning`
- `4ccdb49880` `fix(codex-rs/core/src/hook_runtime.rs): timing`

Expected result:

- hook behavior is config-driven rather than stitched in ad hoc
- runtime memory surfaces are explicit and internally consistent
- observe payload capture and context planning match the intended contract

Target cleanup shape: `2` to `4` commits.

### Lane 3. Mission, handoff, review, and retrieval trace

Replay next:

- `6fdfedb6cf` `feat: emit assistant_result agentmemory events`
- `7ea442afd0` `feat: preserve agentmemory retrieval trace summaries`
- `89589583bb` `feat: add agentmemory mission and handoff surfaces`
- `ee5d223b40` `test: cover agentmemory mission and handoff adapter calls`
- `1a831a30b7` `feat: expose expanded agentmemory review surfaces`
- `09b8584612` `feat: tighten handoff resume and memory rendering`
- `0045a57417` `core: auto-review resume handoff packets`
- `f97a2cd05a` `Add TUI coverage for automatic handoff replay`

Expected result:

- handoff packets survive resume paths
- mission/review surfaces are queryable and renderable
- retrieval traces and assistant-result events survive replay

Target cleanup shape: `2` to `4` commits.

### Lane 4. TUI and app-server parity

Port onto current upstream layout instead of trusting raw replay fixes:

- `5312255dac` `feat: replay source-aware memory ui`
- `57fa4281c6` `Restore TUI memory slash commands`
- `f872688482` `Fix TUI memory footer wiring`
- `9ec683dc46` `fix: resolve post-rebase tool and slash dispatch fallout`
- `fd2f43a943` `chore: refresh app-server schema fixtures after rebase`

Expected result:

- TUI slash commands, footer, history, and replay views work on the current
  upstream TUI layout
- app-server and protocol shapes stay aligned with rendered UI behavior

Target cleanup shape: `2` to `3` commits.

### Lane 5. Docs and fork operations

Replay after code shape is stable:

- `dc2ee2b1d6` `docs: propose agentmemory context optimization`
- `dd0b51d211` `docs: align agentmemory proposal with backend retrieval`
- `f4ea95093f` `docs: make agentmemory proposal aggressively opt-in`
- `a47235d20e` `docs: tighten agentmemory proposal to match intent`
- `03ed54d885` `docs: clarify aggressive context injection semantics`
- `6f023a242c` `docs: add agentmemory payload follow-up spec`
- `099d8b0e91` `docs: add mission and handoff follow-up spec`
- `2ae086845f` `docs: add slash command usage spec`
- `cb42e63c4a` `docs: add remaining agentmemory hardening spec`
- `5e51e376b0` `docs: replay fork policy and public source guidance`
- `4c795fe0d7` `docs: record replay tail assessment`
- `c49859241a` `docs: add replay workflow note`
- `517b893059` `docs: record narrow replay leftover check`

Special rule:

- `a984c97ce7` `Remove GitHub Actions workflows` is not a direct replay target
  and must be re-authored only if still justified after inspecting the current
  upstream workflow set

Target cleanup shape: `2` to `5` commits.

### Lane 6. Optional perf/build follow-up

Replay only if still useful after upstream sync:

- `d9392c3395` `build(just): prune perf build targets`
- `400634b13c` `build(just): add local perf build recipes`
- `677176431c` `fix(just): avoid empty array expansion in perf recipes`
- `c0f1b1b45b` `Fix perf PGO llvm-profdata lookup`

This lane is optional and should not block the core upstream sync.

## Upstream Behavior That Must Be Preserved

Starting from `upstream/main` already includes these changes, but the replay
must not accidentally regress them:

- `2c59806fe0` memory-usage metrics
- `e5b52a3caa` persisted and prewarmed agent tasks per thread
- `370bed4bf4` trust-gated project hooks and exec policies
- `fc758af9eb` sub-agent exec policy loading fix
- `be4fe9f9b2` `--ignore-user-config` and `--ignore-rules`
- `7995c66032` streamed `apply_patch` changes
- `8494e5bd7b` permission-request hooks support
- `d9c71d41a9` OTEL hook-run metrics
- `917a85b0d6` queued slash and shell prompts in the TUI
- `241136b0e9` plan prompt context usage in the TUI
- `ce0e28ea6f` reduced redundant memory notice behavior

## Keep, Squash, Or Drop Guidance

### Keep as replay sources

Use these as primary sources of behavior:

- the lane commits listed above with descriptive messages
- docs commits that encode actual product intent, not just narration

### Squash into owning lanes

These look like fallout from prior replay attempts and should not survive as
standalone commits on the new branch:

- `69fd31b72e` compile regression repair
- `4ccdb49880` hook runtime timing fix
- `9ec683dc46` post-rebase slash/tool fallout
- `fd2f43a943` schema fixture refresh
- `c14d6d462e` config import fix

### Review manually before carrying forward

These are too opaque to replay blindly:

- `f7bba8d79d` `fix`
- `527e956008` `fix buidl`
- `6c1e72d853` `fix: unblock codex-cli release build`
- `98e723f6f1` `127.0.0.1 rather than localhost due to ipv4/v6`

Each one needs a concrete behavioral reason before it is reapplied.

### Drop

Do not replay these as standalone work:

- `2da8ade8c8` formatting cleanup
- `3fdcecdd36` import normalization

## Top Manual Conflict Magnets

Expect hand merges here:

1. `codex-rs/core/src/hook_runtime.rs`
2. `codex-rs/core/src/codex.rs`
3. `codex-rs/core/src/tools/handlers/memory_runtime.rs`
4. `codex-rs/core/src/agentmemory/mod.rs`
5. `codex-rs/core/src/tools/spec.rs`
6. `codex-rs/core/src/tools/registry.rs`
7. `codex-rs/app-server-protocol/src/protocol/v2.rs`
8. `codex-rs/tui/src/app.rs`
9. `codex-rs/tui/src/chatwidget.rs`
10. `codex-rs/tui/src/history_cell.rs`
11. `codex-rs/tui/src/chatwidget/slash_dispatch.rs`
12. `codex-rs/tui/src/slash_command.rs`
13. `codex-rs/tui/src/bottom_pane/chat_composer.rs`
14. `codex-rs/tui/src/bottom_pane/command_popup.rs`

## Replay Mechanics

Preferred transport by situation:

- clean self-contained change: `git cherry-pick -x <sha>`
- similar file shape but patch drift: `git format-patch -1 <sha> --stdout | git apply -3`
- same intent, different architecture: semantic backport with a new commit

Rule: do not treat successful patch application as proof that the replay is
correct. Always validate behavior against the owning lane.

## Validation Plan

Minimum validation per lane:

- run the smallest relevant Rust crate tests for touched code
- refresh and inspect generated schema outputs when protocol shapes move
- run TUI snapshot tests when rendered output changes
- verify slash-command and history/replay behavior when memory UI changes

Suggested checkpoints:

1. after Lane 1: `cargo test -p codex-core`
2. after Lane 2: `cargo test -p codex-core`
3. after Lane 3: `cargo test -p codex-core`
4. after Lane 4: `cargo test -p codex-tui`
5. after protocol/app-server changes:
   `cargo test -p codex-app-server-protocol`

If common, core, or protocol change broadly enough to justify a full-suite run,
ask before running workspace-wide tests.

## Definition Of Done

The rebase effort is complete when:

- the fork behavior is replayed on top of `upstream/main`
- the new branch history is materially cleaner than the current `62`-commit
  fork stack
- the core agentmemory, hook, protocol, and TUI behaviors are preserved
- the critical upstream improvements listed above are still present
- any remaining drift is documented explicitly rather than hidden in fixup
  commits

## Recommended Next Step

Create a throwaway execution branch from `upstream/main` and replay Lane 1
first. Do not start with the full fork stack.
