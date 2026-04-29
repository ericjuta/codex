# Upstream Maintenance Candidates

Snapshot date: 2026-04-28

Branch: `scratch/upstream-worthwhile-20260427`

Local head: `a6bc341557`

Upstream compared: `openai/main` at `fa127be25f`

Divergence at snapshot: local branch is 57 commits ahead and 303 commits
behind `openai/main`.

This branch remains a selective upstream-pick branch. Do not do a full rebase
only to pick up maintenance fixes; the upstream delta still includes broad
permission/profile, memory-split, config-loader, MCP split, app-server handler,
and UI refactor work that should stay in dedicated lanes.

## Already Absorbed

The previous 2026-04-26 candidate list is mostly landed on this branch. Recent
absorbed OpenAI upstream picks include:

- `b7fec54354` Queue follow-up input during user shell commands (#18820)
- `72f757d144` Increase app-server WebSocket outbound buffer (#19246)
- `79ea577156` TUI: Keep remote app-server events draining (#18932)
- `f8c527e529` multi_agent_v2: move thread cap into feature config (#19792)
- `491a3058f6` exec-server retain output until streams close (#18946)
- `8f0a92c1e5` Fix relative stdio MCP cwd fallback (#19031)
- `6c51bf0c7c` Hide rewind preview when no user message exists (#19510)
- `8033b6a449` Add /auto-review-denials retry approval flow (#19058)
- `11e5af53c4` Add plumbing to approve stored Auto-Review denials (#18955)
- `0bd25ab374` Delay approval prompts while typing (#19513)

## Next Recommended Picks

1. `92fb848065` Allow large remote app-server resume responses (#19920)
   - Value: raises the remote app-server client receive path so large resume
     and thread payloads do not fail before the caller can process them.
   - Scope: `codex-rs/app-server-client/src/lib.rs` and
     `codex-rs/app-server-client/src/remote.rs`.
   - Applicability: local app-server client code does not appear to have this
     receive-size handling yet.
   - Recommendation: take first.

2. `fd36838cf3` Add MultiAgentV2 root and subagent context hints (#19805)
   - Value: improves root and subagent prompt context for multi-agent v2, which
     this branch already carries and configures.
   - Scope: `codex-rs/core/src/agent/control.rs`,
     `codex-rs/core/src/session/mod.rs`, `codex-rs/core/src/session/multi_agents.rs`,
     config schema, feature config, and tests.
   - Applicability: local branch has multi-agent v2 config and usage-hint
     support but not the upstream root/subagent context hint layer.
   - Recommendation: take second if the branch remains focused on multi-agent
     quality.

3. `85c1500569` Filter dynamic deferred tools from model_visible_specs
   (#19771)
   - Value: prevents dynamic deferred tools from leaking into
     `ToolRouter::model_visible_specs`, including compaction request payloads
     that reuse those specs.
   - Scope: `codex-rs/core/src/session/turn.rs`,
     `codex-rs/core/src/tools/router.rs`, router tests, and compact/search
     integration tests.
   - Applicability: local branch has adjacent dynamic/deferred tool plumbing,
     so expect manual reconciliation in core.
   - Recommendation: high-value but core-riskier; take with focused core
     compact/search coverage.

4. `5ba908d179` Avoid persisting ShutdownComplete after thread shutdown
   (#19630)
   - Value: avoids writing a shutdown-complete event after the thread is already
     shut down.
   - Scope: `codex-rs/core/src/session/handlers.rs` and
     `codex-rs/core/src/session/tests.rs`.
   - Applicability: local code still has the relevant `ShutdownComplete`
     handling path.
   - Recommendation: small correctness pick; take after the higher-value
     app-server and multi-agent picks.

5. `2307aa8d98` Allow /statusline and /title slash commands during active
   turns (#19917)
   - Value: lets users adjust status-line and terminal-title surfaces without
     waiting for an active turn to finish.
   - Scope: `codex-rs/tui/src/slash_command.rs`.
   - Applicability: local TUI has these slash commands and active-turn command
     gating.
   - Recommendation: small TUI ergonomics pick.

6. `52c06b8759` Preserve TUI markdown list spacing after code blocks (#19706)
   - Value: preserves readable markdown list spacing after fenced code blocks.
   - Scope: `codex-rs/tui/src/markdown_render.rs`,
     `codex-rs/tui/src/markdown_render_tests.rs`, and snapshots.
   - Applicability: standalone TUI rendering fix.
   - Recommendation: low-risk UI polish if TUI snapshots are already in scope.

7. `277186ec85` Cap original-detail image token estimates (#19865)
   - Value: clamps original-detail image patch estimates to the current 10k
     patch budget so large images cannot inflate local context accounting
     without bound.
   - Scope: `codex-rs/core/src/context_manager/history.rs` and
     `codex-rs/core/src/context_manager/history_tests.rs`.
   - Applicability: local branch already has inline image-data-url estimate
     hardening; verify whether this exact cap is already covered before
     cherry-picking.
   - Recommendation: patch-equivalence check first, then take only if missing.

## Defer For Dedicated Lanes

- `5591912f0b` TUI resize scrollback reflow (#18575)
  - Good user-visible behavior, but about 40 files and thousands of changed
    lines. Needs a dedicated TUI lane with snapshot review.

- `1e560f33e1` Compress skill paths with root aliases (#19098)
  - Good context-budget work, but large skill-rendering/context plumbing
    surface. Needs a dedicated context-budget lane.

- `0a9b559c0b` Migrate fork and resume reads to thread store (#18900)
  - Important architectural direction, but high overlap with local
    agentmemory/resume/fork behavior. Needs a dedicated integration branch.

- Permissions/profile series around #19392 through #19737
  - Broad cross-crate policy migration. Too wide for the current maintenance
    branch.

- Memory split series around #18982, #19818, #19860, and #19967
  - Useful direction, but it moves large memory/runtime surfaces out of
    `codex-core`. Too wide for this maintenance branch and likely to overlap
    local memory/runtime work.

- `9c3abcd46c` Move config loading into codex-config (#19487)
  - Good crate-boundary cleanup, but about 70 files and high overlap with
    config-policy work. Needs a dedicated config lane.

- `0bda8161a2` Split MCP connection modules (#19725)
  - Good architecture, but moves thousands of lines across MCP modules. Needs a
    dedicated MCP lane.

- App-server handler streamlining series around #19490 through #19498
  - Valuable cleanup, but broad request-handler churn. Take only in an
    app-server-focused branch.

- Remote plugin install/uninstall bundle caching around #19456 and #19914
  - Product value depends on active remote-plugin work. The bundle-cache commit
    also changes dependencies and Bazel lock state, so it is not a cheap
    maintenance pick.

- `4e30281a13` Guard npm update readiness (#19389)
  - Lower priority here and does not apply cleanly because this fork removed
    the relevant GitHub workflow file.

- `d19de6d150` Bedrock GPT-5.4 reasoning levels (#19461)
  - Not directly applicable to this checkout; it touches a Bedrock provider
    path that is not present in the current branch.

- `687c5d9081` Update unix socket transport to use WebSocket upgrade (#19244)
  - Does not apply to the current app-server transport layout; revisit only if
    unix-socket transport compatibility becomes active work.

## Current Takeaway

Do not full rebase. Current upstream is 303 commits ahead and includes several
large architectural lanes. The best next selective-pick order is:

1. `92fb848065` large remote app-server resume responses.
2. `fd36838cf3` multi-agent v2 root/subagent context hints.
3. `85c1500569` dynamic deferred tool filtering.
4. `5ba908d179` shutdown-complete persistence guard.
5. `2307aa8d98`, `52c06b8759`, and maybe `277186ec85` as small TUI/core
   polish once patch-equivalence is checked.
