# Upstream Replay Plan

Date: 2026-03-31

Current scratch base: `25fbd7e40e` (`openai/codex` `main`)

Fork point from the prior fork branch: `b00a05c785`

## Goal

Rebase the fork-specific work onto current upstream while cleaning up the fork
history. The replay should preserve the intentional fork lanes:

- agentmemory as the primary memory backend
- expanded hook and memory event surface
- memory UX parity in the TUI
- fork-intent and public-release documentation
- private-fork CI posture
- local perf/build improvements that still make sense on top of upstream

## Replay Strategy

Do not run a blind linear rebase of all fork commits.

Instead:

1. Start from current upstream `main` on this scratch branch.
2. Replay the fork work by lane.
3. Squash the fork stack down to a smaller set of meaningful commits.
4. Drop tracked patch/orig artifacts and formatting-only noise.
5. Re-author the private-fork workflow trim on top of the current upstream
   workflow set instead of cherry-picking the old deletion commit.

## Lane Order

### 1. Core agentmemory and hook contract lane

Replay first:

- `1a8eebeef2` feat: introduce memory backend selector and agentmemory adapter seam
- `620fe2a782` feat: replace startup memory prompt generation with agentmemory-backed retrieval
- `00b2411de3` feat: expand Codex public hooks to support the full agentmemory event model
- `3eab67919e` feat: emit tool usage events from high-signal tools for agentmemory
- `556c08d35d` feat: capture and store lifecycle events in agentmemory
- `0e0a5545b7` feat: bypass native memory generation when agentmemory is enabled
- `4fe579325c` feat: ensure memory commands pass through to agentmemory natively without bridging
- `6eb14dc021` feat: integrate agentmemory into CLI components and workflows
- `78281454bd` test: add agentmemory adapter unit tests
- `43bb23a7a4` fix: rename memory slash commands and make them visible builtins
- `0d47d05e56` fix: correctly show memory slash commands when agentmemory is enabled
- `ee99969de6` Improve Agentmemory session payloads
- `01e9eefbc8` Implement agentmemory payload quality spec
- `0c220fb634` feat: add mid-session memory recall and streaming assistant capture
- `c5ef3dae76` Fix agentmemory session lifecycle

Expected shape after cleanup: 3 to 5 commits instead of the original stack.

### 2. Runtime memory surface lane

Replay after the backend contract is stable:

- `063e8ea819` Add agentmemory runtime recall surface
- `09311f040f` Refine proactive agentmemory recall guidance
- `713dbb3804` Add structured memory operation events

Target shape after replay:

- explicit human/assistant recall
- explicit remember surface
- read-oriented lessons/crystals/insights surface
- action list/frontier/next surface, with mutation support if the lane carries it

Expected shape after cleanup: 1 to 2 commits.

### 3. TUI and app-server parity lane

Port onto the upstream layout rather than trusting raw cherry-picks:

- `dc8a2a82a6` tui app server parity
- `953d2b70c4` Clean app-server memory parity follow-up
- `41ab03969b` Surface app-server TUI thread op failures
- `7a9e3b1a23` Add visual memory history cells
- `c23f19e387` Use structured memory events in the TUIs
- `f0c1b3ccec` Finish memory UI follow-up lane

Expected shape after cleanup: 2 to 4 commits.

### 4. Lint/build cleanup lane

Replay only if still needed after the code lands:

- `412c559144` Fix argument comment lint in codex core
- `b93aba4fab` Fix argument-comment-lint invocation from codex-rs

The current upstream already changed lint tooling and CI substantially, so
these may become obsolete during replay.

### 5. Fork-intent and private-fork operations lane

Keep the policy/docs lane, but re-author the workflow trim:

- `3952eb4e77` Add private fork GitHub Actions spec
- re-author the intent of `4b42fc0216` on top of current upstream workflows
- `7637de88e9` docs: clarify public source licensing and fork intent
- `8573eaea33` docs: split fork intent from release notes

Expected shape after cleanup: 2 to 3 commits.

### 6. Fork-local perf/build lane

Replay last:

- `9a6bcdaf00` perf(cli): add optional mimalloc allocator
- `90ee68291f` build(just): add perf-build-local recipe

## Commits To Squash Or Drop

### Squash into docs or omit until the end

These are useful for provenance but too granular for the replayed branch:

- `988b868cfe`
- `3539af3930`
- `40774ed67d`
- `6c5094f86e`
- `fb371104a0`
- `0d6d705340`
- `3792d2fb8b`
- `2cc63f9130`
- `5b5af2bb60`
- `2cf9895352`
- `fbad88c53f`

### Drop

- `cecb9ae79c` style: apply rustfmt formatting

Also drop the tracked patch/orig artifacts instead of carrying them forward:

- `patch3.diff`
- `patch_tests.diff`
- `patch_agentmemory.diff`
- `codex-rs/core/src/agentmemory/mod.rs.orig`
- `codex-rs/core/src/agentmemory/mod.rs.patch`
- `codex-rs/core/src/hook_runtime.rs.patch`

## Upstream Conflict Magnets

The highest-risk upstream changes to absorb during replay are:

- `d65deec617` Remove the legacy TUI split
- `61429a6c10` Rename `tui_app_server` to `tui`
- the `codex-tools` extraction series
- `21a03f1671` app-server-protocol introduce generic ClientResponse
- `213756c9ab` feat: add mailbox concept for wait
- the argument-comment-lint and workflow changes around `fce0f76d57`,
  `5037a2d199`, `19f0d196d1`, `9313c49e4c`, `b94366441e`, and `f4f6eca871`

## TUI Port Map

The old fork branch changed both `codex-rs/tui` and
`codex-rs/tui_app_server`. Current upstream no longer has
`codex-rs/tui_app_server`; its files were moved into `codex-rs/tui`.

Likely direct mappings:

- `codex-rs/tui_app_server/src/app.rs` -> `codex-rs/tui/src/app.rs`
- `codex-rs/tui_app_server/src/app_command.rs` -> `codex-rs/tui/src/app_command.rs`
- `codex-rs/tui_app_server/src/app_server_session.rs` -> `codex-rs/tui/src/app_server_session.rs`
- `codex-rs/tui_app_server/src/app/app_server_adapter.rs` -> `codex-rs/tui/src/app/app_server_adapter.rs`
- `codex-rs/tui_app_server/src/chatwidget.rs` -> `codex-rs/tui/src/chatwidget.rs`
- `codex-rs/tui_app_server/src/history_cell.rs` -> `codex-rs/tui/src/history_cell.rs`
- `codex-rs/tui_app_server/src/slash_command.rs` -> `codex-rs/tui/src/slash_command.rs`
- `codex-rs/tui_app_server/src/bottom_pane/chat_composer.rs` -> `codex-rs/tui/src/bottom_pane/chat_composer.rs`
- `codex-rs/tui_app_server/src/bottom_pane/command_popup.rs` -> `codex-rs/tui/src/bottom_pane/command_popup.rs`
- `codex-rs/tui_app_server/src/bottom_pane/mod.rs` -> `codex-rs/tui/src/bottom_pane/mod.rs`
- `codex-rs/tui_app_server/src/bottom_pane/slash_commands.rs` -> `codex-rs/tui/src/bottom_pane/slash_commands.rs`

## Top Manual Conflict Spots

1. `codex-rs/tui/src/app.rs`
2. `codex-rs/tui/src/chatwidget.rs`
3. `codex-rs/tui/src/history_cell.rs`
4. `codex-rs/tui/src/slash_command.rs`
5. `codex-rs/tui/src/bottom_pane/chat_composer.rs`
6. `codex-rs/tui/src/bottom_pane/command_popup.rs`
7. `codex-rs/tui/src/bottom_pane/mod.rs`
8. `codex-rs/tui/src/bottom_pane/slash_commands.rs`
9. `codex-rs/app-server-protocol/src/protocol/v2.rs`
10. `codex-rs/core/src/codex.rs`

## Validation Goal

Definition of done for the scratch replay branch:

- the intended fork lanes are replayed on top of current upstream
- the branch history is materially cleaner than the old 44-commit fork stack
- tracked patch/orig artifacts are gone
- targeted validation passes for the touched Rust crates
- remaining conflict debt, if any, is documented explicitly
