# Upstream Maintenance Candidates

Snapshot date: 2026-04-27

Branch: `scratch/upstream-worthwhile-20260427`

Local head: `b0f5f1733a`

Upstream compared: `openai/main` at `0bd25ab374`

Divergence at snapshot: local branch is 56 commits ahead and 265 commits
behind `openai/main`.

This branch remains a selective upstream-pick branch. Do not do a full rebase
only to pick up maintenance fixes; the upstream delta still includes broad
permission/profile, thread-store, CI, and UI refactor work that should stay in
dedicated lanes.

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

## Next Recommended Picks

1. `0bd25ab374` Delay approval prompts while typing (#19513)
   - Value: prevents typed-ahead composer input such as `y` or `a` from
     being consumed as approval shortcuts when an approval modal appears while
     the user is still typing.
   - Scope: `codex-rs/tui/src/bottom_pane/approval_overlay.rs` and
     `codex-rs/tui/src/bottom_pane/mod.rs`.
   - Dry-run result: clean 3-way apply.
   - Recommendation: take first.

2. `277186ec85` Cap original-detail image token estimates (#19865)
   - Value: clamps original-detail image patch estimates to the current 10k
     patch budget so large images cannot inflate local context accounting
     without bound.
   - Scope: `codex-rs/core/src/context_manager/history.rs` and
     `codex-rs/core/src/context_manager/history_tests.rs`.
   - Dry-run result: clean 3-way apply.
   - Recommendation: take second; run focused core context-manager tests.

3. `1f304dd1f2` Allow agents.max_threads to work with multi_agent_v2 (#19733)
   - Value: keeps the existing `agents.max_threads` setting effective after
     the branch absorbed the upstream `multi_agent_v2` feature-config move.
   - Scope: `codex-rs/core/src/config/mod.rs`.
   - Dry-run result: 3-way apply reports a small conflict in
     `codex-rs/core/src/config/mod.rs`.
   - Recommendation: manually port after the two clean picks; run focused
     config tests.

4. `850f035b8c` Fix filtered thread-list resume regression in TUI (#19591)
   - Value: avoids unnecessary full JSONL rollout reads for filtered TUI resume
     listings while preserving the correctness-preserving filesystem-backed
     read-repair path.
   - Scope: `codex-rs/rollout/src/recorder.rs`.
   - Dry-run result: 3-way apply reports a one-file conflict in
     `codex-rs/rollout/src/recorder.rs`.
   - Recommendation: manually port if resume/thread-list latency is in scope;
     run `cargo test -p codex-rollout list_threads`.

5. `85c1500569` Filter dynamic deferred tools from model_visible_specs
   (#19771)
   - Value: prevents dynamic deferred tools from leaking into
     `ToolRouter::model_visible_specs`, including compaction request payloads
     that reuse those specs.
   - Scope: `codex-rs/core/src/session/turn.rs`,
     `codex-rs/core/src/tools/router.rs`, router tests, and compact/search
     integration tests.
   - Dry-run result: most files apply, but
     `codex-rs/core/src/session/turn.rs` needs manual reconciliation.
   - Recommendation: worthwhile but core-riskier; take only with enough time
     for focused core tests and compact/search coverage.

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

Take the two clean picks first:

1. `0bd25ab374` approval prompt delay.
2. `277186ec85` original-detail image token cap.

Then decide whether to spend a manual-port pass on:

1. `1f304dd1f2` `agents.max_threads` with `multi_agent_v2`.
2. `850f035b8c` filtered resume listing repair.
3. `85c1500569` dynamic deferred tool filtering.
