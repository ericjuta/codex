# Replay Tail Assessment

## Branch

`scratch/replay-enact-openai-20260401`

## Purpose

Record what was intentionally left out after replaying the fork work onto `openai/main`, and note the new upstream drift that appeared after the replay pass.

## Replayed Commits

The replay branch currently carries these grouped replay commits:

1. `e05798082` - `feat: replay fork pattern v2 coverage`
2. `26e9f5071` - `feat: replay agentmemory backend surface`
3. `c778ed111` - `feat: replay structured memory operation events`
4. `4656405a5` - `feat: replay source-aware memory ui`
5. `0185a0558` - `docs: replay fork policy and public source guidance`
6. `313772428` - `ci: replay private fork workflow baseline`

## Intentionally Skipped

The original replay branch ended with:

1. `8c2cb3df5` - `fix(replay): finish macos tui-cli integration`

This commit was intentionally not replayed as-is.

## Why `8c2cb3df5` Was Not Replayed

On top of the new replay branch, `8c2cb3df5` no longer behaves like a small cleanup or integration fix.

Compared against the replay branch, it expands into a large mixed diff:

- touches 142 files
- mixes product code, analytics refactors, auth changes, app-server changes, TUI test layout reversions, workflow/script churn, and lockfile drift
- includes areas already intentionally resolved differently during the fresh replay

Replaying it directly would undo the clean branch shape created during the grouped replay and would likely reintroduce upstream-diverged churn.

## Practical Reading of `8c2cb3df5`

Treat `8c2cb3df5` as an old catch-all integration bucket, not as a replay-safe leaf commit.

If any useful deltas remain inside it, they should be extracted surgically into new small commits instead of replaying the original commit wholesale.

Likely candidates for surgical extraction, if needed later:

1. small CLI entrypoint glue
2. small TUI footer or slash-command wiring changes
3. narrowly scoped protocol fixture updates that are still missing

Do not use the original commit as the source of truth for:

1. analytics client structure
2. auth refactors
3. test module layout
4. broad workflow or script changes

## Narrow Leftover Check

A follow-up narrow pass checked the likely small integration-glue files that would have been the only realistic candidates for surgical extraction from `8c2cb3df5`.

That pass did not find meaningful remaining deltas in the replay branch for:

1. CLI entrypoint glue
2. slash-command wiring
3. bottom-pane or command-popup integration glue
4. core tool registry or tool spec glue
5. protocol schema fixture files tied to the replayed memory and hook surfaces
6. TUI status/footer snapshot surfaces that looked plausibly replay-related

Practical conclusion:

There is no obvious small product fix still hiding inside `8c2cb3df5` that needs to be extracted immediately. The remaining differences between the old scratch branch and the replay branch are best understood as upstream divergence, policy churn, test-layout churn, or unrelated subsystem changes rather than a missing replay-critical integration patch.

## Upstream Drift After Replay

After the replay pass, `openai/main` moved again.

At the time this note was first written:

- replay branch: `scratch/replay-enact-openai-20260401`
- upstream head: `c846a57d032b`
- relation: `ahead 6`, `behind 6`

The upstream-only commits at that point were:

1. `3152d1a55` - `Use message string in v2 assign_task (#16419)`
2. `0c776c433` - `feat: tasks can't be assigned to root agent (#16424)`
3. `df5f79da3` - `nit: update wait v2 desc (#16425)`
4. `609ac0c7a` - `chore: interrupted as state (#16426)`
5. `5bbfee69b` - `nit: deny field v2 (#16427)`
6. `c846a57d0` - `chore: drop log DB (#16433)`

Those upstream commits have since been absorbed into the replay branch by cherry-pick-equivalent commits, so they should no longer be treated as pending replay work.

## Upstream Drift Status After Absorbing The Six Commits

The replay branch now includes patch-equivalent versions of:

1. `3152d1a55` - `Use message string in v2 assign_task (#16419)`
2. `0c776c433` - `feat: tasks can't be assigned to root agent (#16424)`
3. `df5f79da3` - `nit: update wait v2 desc (#16425)`
4. `609ac0c7a` - `chore: interrupted as state (#16426)`
5. `5bbfee69b` - `nit: deny field v2 (#16427)`
6. `c846a57d0` - `chore: drop log DB (#16433)`

Because they were cherry-picked, raw ahead/behind counts still show divergence by SHA, but `git cherry` confirms these upstream patches are represented on the replay branch.

## Recommendation

Before any second replay pass, reassess the new upstream multi-agent commits first.

In particular:

1. `3152d1a55` matters because it continues the v2 message-shape changes already seen in `send_message` and `spawn_agent`
2. `0c776c433` matters because it changes task-routing semantics and may affect assumptions in replayed fork or memory flows
3. `c846a57d0` matters because the replay branch currently includes agentmemory and structured memory operation work, while upstream is actively changing log DB behavior

If a follow-up replay is needed, absorb these upstream commits first and only then extract any remaining tiny deltas from `8c2cb3df5`.

## 2026-04-07 Upstream Integration Inventory

After a fresh comparison against `openai/codex` `main` on 2026-04-07, the
branch was behind by 30 upstream commits. A blind rebase was not considered
safe because upstream and fork work now overlap in:

1. thread/app-server protocol fields
2. core session startup and system-context injection
3. tool/runtime configuration plumbing
4. TUI history and slash-command surfaces

### Absorbed Now

1. `24c598e8a9` - `Honor null thread instructions (#16964)`

This upstream patch is aligned with the fork intent because it fixes a real
semantic gap in the thread-start / resume / fork path:

1. omitted instruction fields continue to mean "inherit or fall back"
2. explicit `null` now means "blank-slate override"
3. explicit empty strings remain distinct from `null`

That matters directly for:

1. assistant-visible system/base instruction injection
2. app-server request semantics for thread lifecycle operations
3. replayed session metadata and base-instruction restoration
4. spawned/forked runtime behavior when the fork intentionally clears model guidance

### Deferred On Purpose

1. `4bb507d2c4` - `Make AGENTS.md discovery FS-aware (#15826)`
2. `9f737c28dd` - `Speed up /mcp inventory listing (#16831)`
3. `756c45ec61` - `[codex-analytics] add protocol-native turn timestamps (#16638)`
4. `1f2411629f` - `Refactor config types into a separate crate (#16962)`
5. `73dab2046f` - `app-server: Add transport for remote control (#15951)`

These were deferred because they either:

1. create broad churn outside the current fork lanes
2. touch high-conflict TUI or config surfaces without closing a correctness gap
3. add platform or product surface area that this fork is not trying to replay right now

### Practical Outcome

The current branch should treat null-instruction handling as required upstream
correctness debt that belongs inside the replayed fork stack. The remaining
upstream commits should be considered in later narrow passes by lane rather
than through another undifferentiated rebase attempt.
