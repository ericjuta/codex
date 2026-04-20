# Replay Workflow

## Goal

Provide a repeatable process for replaying fork-only work onto a moving `openai/main` without turning branch drift into unnecessary conflict churn.

## Default Strategy

Do not keep healing an old replay branch once upstream has moved significantly in the same feature area.

Start a fresh branch from current `openai/main`, then replay the fork-only work in grouped commits by seam.

## Preferred Replay Shape

Group replay work by behavior and ownership:

1. core or backend feature plumbing
2. protocol or app-server event surface
3. TUI or UI behavior
4. docs and policy
5. workflow policy

Do not mix these into one catch-all replay commit.

## Rules

1. Keep policy commits separate from product commits.
2. Keep workflow trimming separate from docs.
3. Keep generated files with the feature commit that owns them.
4. Avoid final “cleanup” or “finish replay fallout” commits.
5. If a replay commit explodes into unrelated churn on a fresh branch, stop and extract only the tiny deltas that still matter.

## Why This Works

Fresh replay reduces conflict surface because:

1. upstream-only refactors are already present in the new base
2. local conflict resolution happens once per seam instead of once per historical step
3. branch intent stays visible in review
4. policy disagreements do not get mixed into runtime code merges

## Recommended Order

1. fetch current `openai/main`
2. branch fresh from upstream
3. replay isolated semantic commits first
4. replay backend and protocol feature work
5. replay TUI or UI work
6. replay docs and policy
7. replay workflow policy last
8. reassess upstream drift before another replay pass

## Upstream Drift Handling

When upstream keeps moving during the replay:

1. check the new upstream-only commits
2. absorb the ones that touch the same semantic surface first
3. continue replay only after the active conflict seam is up to date

Raw ahead or behind counts can mislead after cherry-picks. Use patch-equivalence checks such as `git cherry` when verifying whether upstream changes are already represented on the replay branch.

## What To Avoid

Avoid using an old catch-all replay commit as the source of truth.

Those commits usually accumulate:

1. generated-file churn
2. workflow churn
3. test-layout churn
4. unrelated refactors that only happened to be nearby

If one of those commits still matters, mine it surgically instead of replaying it wholesale.

## Good Outcome

A good replay branch is:

1. feature-complete for the intended fork behavior
2. current enough with upstream that active semantic conflicts are already absorbed
3. cleaner than the old scratch branch
4. intentionally not a diff-identical copy of all historical branch churn
