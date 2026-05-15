# Agent Session Operations Spec

This spec defines the desired operating loop for Codex sessions that need to
move through repo work, host configuration, runtime services, memory context,
and pull-request follow-up without losing track of the current truth.

The intent is not to add a new runtime feature. The intent is to make future
sessions predictable: begin from live state, name the active lane, keep work
visible, verify the result through the surface that matters, and close with a
compact handoff.

## Goals

- Start every non-trivial turn from a small live-state readback instead of
  relying on remembered context alone.
- Keep repo source, host configuration, runtime services, memory context, and
  publication state separate until there is direct proof that connects them.
- Use the plan tool as the visible work ledger throughout multi-step tasks.
- Delegate independent reads, implementation slices, test diagnosis, and review
  work to focused subagents while keeping final decisions in the root session.
- Use explicit command timeouts for commands that are expected to be slow while
  keeping simple read commands fast.
- End each substantial turn with the smallest useful proof summary and one clear
  next move.

## Non-Goals

- Do not change Codex runtime scheduling, tool semantics, or model provider
  behavior through this spec.
- Do not use this spec as a replacement for live repo, process, network, test,
  or git verification.
- Do not add broad product documentation or user-facing docs from this work.
- Do not encode host-specific secrets, private paths that contain credentials, or
  raw secret-bearing logs in session artifacts.
- Do not treat memory context, injected context, or prior summaries as current
  truth without checking the relevant live surface.

## Session Start Card

For a non-trivial task, the session should establish a compact state card before
making changes:

```text
cwd:
branch:
dirty state:
remote / publication target:
lane:
applicable instructions:
proof surface:
done condition:
```

The card does not need to be verbose or user-visible in every turn, but the
agent should collect enough of it to avoid editing the wrong surface. Typical
commands are `pwd`, `git status --short --branch`, `git remote -v`, applicable
`AGENTS.md` reads, and the narrowest command or file read that proves the
reported problem.

## Lane Classification

Every task should identify its primary lane before mutation:

| Lane | Truth surface | Typical proof |
| --- | --- | --- |
| Repo code | Source tree and tests | Focused diff, formatter, crate or package tests |
| Host config | Files under `~/.codex` | Live file readback and feature registry checks |
| Runtime service | Running process, container, socket, or API | Health endpoint, logs, listener, process tree |
| Memory context | Honcho or memory files | Source-labeled retrieval plus live verification |
| Publication | Git remotes, branches, PRs, CI | `git status`, `git ls-remote`, PR checks, workflow state |

When a task crosses lanes, the answer should say which lane was changed and
which lane was only inspected. For example, editing `~/.codex/AGENTS.md` changes
agent guidance, not Codex runtime behavior; changing repo source may still need
a rebuild or deployment before it affects a running service.

## Plan Discipline

The plan is the session's work ledger:

- Start a plan before multi-step work.
- Keep only one item in progress.
- Update the plan when the task changes lanes, when edits begin, and when
  verification starts.
- Treat terse follow-ups such as `continue`, `.`, `ok check`, `eval`, and
  `health` as instructions to resume from the current live state rather than to
  restart broad discovery.

Plans should stay small. A good default shape is:

1. Inspect live state and constraints.
2. Make the scoped change.
3. Verify the relevant proof surface.
4. Commit, push, or hand off if requested.

## Subagent Discipline

Use subagents when work splits cleanly:

- Independent code or document reading.
- Implementation in bounded files or modules.
- Test failure diagnosis.
- Review of a completed diff.
- Memory or prior-context lookup.

Each subagent should receive an explicit scope, no-go areas, evidence
requirements, and output shape. The root session remains responsible for
decisions, staging, commits, user-visible claims, and live verification of risky
agent conclusions.

Root should also close stale agents once their evidence is collected. A useful
root loop is:

```text
spawn focused agents -> continue independent work -> wait/list agents -> verify risky claims -> close agents
```

## Timeout Policy

Keep omitted shell timeouts short for simple commands. Use explicit `timeout_ms`
for commands that are known or expected to be slow:

- Tests, builds, formatters that may compile, and package installs.
- Cargo, Bazel, Docker, network probes, and service health checks.
- Long-running audits, PR polling, and CI inspection.

Do not raise generic command timeouts to hide hangs. Prefer a deliberate timeout
that matches the command's expected duration.

## Health And Eval Answers

For prompts such as `health`, `working`, `eval`, `quality`, and `maxed`, report
separate proof buckets instead of blending them:

```text
setup wiring:
runtime/API health:
quality/eval signal:
queue freshness:
remaining boundary:
```

A reachable endpoint does not prove useful retrieval quality. A green hook
configuration does not prove that a downstream service is healthy. Queue status
is observability, not by itself a blocker.

## Final Handoff

Substantial turns should close with a compact proof footer:

```text
Changed:
Verified:
Not verified:
Next move:
```

The exact labels are optional, but the answer should make the same information
clear. Prefer direct readbacks, command names, commit hashes, PR URLs, health
endpoints, listener checks, and focused tests over long narrative recaps.

## Rollout

This spec can be adopted as operating guidance immediately. Any later runtime
work should be proposed separately and tied to a concrete behavior gap, such as
agent wake semantics, transcript visibility, or command timeout defaults.
