# Reusable Handoff Prompt: Runtime Codex Sessions (General Session Replay)

Use this prompt in a fresh Codex session when continuing this same workstream.

## 1) Objective
Continue rebasing this fork on origin/main with a narrow focus on operational, day-to-day runtime improvements for Codex sessions, auth flow reliability, and tool/event stream behavior.

## 2) Known local context
- Working directory: /home/ericjuta/.openclaw/workspace/repos/codex
- Branch in use: main (fork branch)
- Upstream remote target: origin (https://github.com/openai/codex)
- Current task is to select and apply only high-impact runtime commits from upstream.

## 3) Selection intent
Prioritize commits that directly affect:
- Session lifecycle and model/request/response orchestration
- Tool-call event visibility and canonical event shapes
- Interleaved response handling and runtime switching behavior
- Auth/session handoff behavior that changes hosted or local auth flows

Avoid:
- Pure refactors with no runtime behavior impact
- CI-only or infra-only maintenance commits
- Documentation-only changes, unless they directly change runtime behavior expectations

## 4) Commit set handling
Use the user-selected upstream IDs exactly, evaluate in ascending order:
1. 2e20d2ef44
2. 7b4e70d567
3. 8917244f7d
4. 7094fa467e
5. 7affe3e3e4
6. b9b934e99b
7. cca16a1087
8. f659eb12bc
9. 1bd9d841ca
10. 058d97c5dc
11. 6b4882528e
12. f1affbac5e
13. ff06ab7172
14. 1fd0858e86
15. a3f8b0b332

For each ID:
- apply only if it changes behavior as above
- if empty or duplicate in current history, mark as already-covered
- if conflict, document exactly why and resolution

## 5) Commands (required order)
1. git fetch origin
2. git rev-list --left-right --count --no-merges HEAD...origin/main
3. git rebase origin/main
   - if full rebase is unsafe, use git cherry-pick <ids in order>
4. git status --short --branch
5. Re-run step 2 until upstream runtime deltas are expectedly reconciled

## 6) Required output for next handoff
- Which selected commits landed cleanly
- Which were already covered as duplicates or empty
- Any conflicts encountered and exact resolution approach
- Whether branch now contains the requested runtime subset
- Explicit note on any non-runtime deltas introduced by mistake

## 7) Operational verification standards
- Distinguish live proof from prior context
- Avoid speculating on status without command-backed evidence
- Keep branch changes minimal; do not alter unrelated code
