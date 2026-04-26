# Upstream Maintenance Candidates

Snapshot date: 2026-04-26

Branch: `scratch/upstream-maintenance-20260426`

Local head: `85c840cab04a`

Upstream compared: `openai/main` at `fed0a8f4faa5`

This branch intentionally carries a focused maintenance subset rather than full
upstream parity. The ranking below is ordered by practical value for this
checkout, with patch risk called out from a live applicability scan.

## Best Candidates

1. `b7fec54354` Queue follow-up input during user shell commands
   - Value: fixes a TUI hang when plain text is submitted while a user shell
     command such as `!sleep 10` is still running.
   - Port risk: clean apply.
   - Recommendation: take first.

2. `3f8c06e457` Fix `/review` interrupt and TUI exit wedges
   - Value: fixes interactive wedged states during `/review` cancellation and
     bounds a TUI shutdown wait.
   - Port risk: small manual port across `codex_delegate` and TUI event
     dispatch.
   - Recommendation: port after the clean TUI shell-queue fix.

3. `72f757d144` Increase app-server WebSocket outbound buffer
   - Value: improves remote app-server robustness during bursts of outbound
     turn and tool-output notifications.
   - Port risk: one-file conflict in WebSocket transport.
   - Recommendation: port with the remote event-drain fix.

4. `79ea577156` Keep remote app-server events draining
   - Value: prevents stale stuck remote TUI state after disconnect or local
     event-channel backpressure.
   - Port risk: one-file conflict in the app-server client remote transport.
   - Recommendation: pair with the WebSocket outbound-buffer fix.

5. `491a3058f6` exec-server retain output until streams close
   - Value: avoids losing stdout/stderr emitted after process exit while pipes
     are still open.
   - Port risk: medium manual port in exec-server process retention and tests.
   - Recommendation: worthwhile after the smaller TUI and remote app-server
     fixes.

6. `8f0a92c1e5` Fix relative stdio MCP cwd fallback
   - Value: improves MCP stdio reliability for relative program resolution.
   - Port risk: medium surface across app-server, codex-mcp, rmcp-client, and
     tests.
   - Recommendation: take as a focused MCP reliability pick.

7. `1e560f33e1` Compress skill paths with root aliases
   - Value: reduces model-visible context cost for long local skill paths.
   - Port risk: larger conflict in skill rendering and context plumbing.
   - Recommendation: dedicated context-budget lane, not a quick pick.

8. `0a9b559c0b` Migrate fork and resume reads to thread store
   - Value: moves fork/resume toward store-backed history instead of direct
     rollout-path reads, which is important for future non-local thread
     storage.
   - Port risk: high because it touches app-server resume/fork, thread-store,
     core session history, and tests near local agentmemory/resume work.
   - Recommendation: dedicated integration branch only.

9. `5591912f0b` TUI resize scrollback reflow
   - Value: good user-visible TUI resize behavior.
   - Port risk: large patch across 40 files with new reflow machinery and
     snapshots.
   - Recommendation: dedicated UI lane.

10. `4e30281a13` Guard npm update readiness
    - Value: prevents update prompts before npm is ready for a release.
    - Port risk: easy if this fork ignores the missing `.github` release
      workflow portion.
    - Recommendation: lower priority unless release/update prompts matter for
      this branch.

## Current Takeaway

The next best sequence is:

1. Cherry-pick `b7fec54354`.
2. Manually port `3f8c06e457`.
3. Port the remote app-server pair `72f757d144` and `79ea577156`.
4. Reassess before taking larger lanes such as exec-server retention, skill path
   compression, TUI resize reflow, or fork/resume thread-store migration.
