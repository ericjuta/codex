<!--
Copy this file to specs/<branch-slug>.md and fill it in. Keep the copied spec
updated as the single source of truth for this branch.

This template adapts OpenAI's ExecPlan guidance from:
https://github.com/openai/openai-cookbook/blob/main/articles/codex_exec_plans.md

The cookbook discusses PLANS.md and ExecPlans. In this repo, an ExecPlan is a
copied specs/*.md file, so keep these sections in the spec instead of creating a
separate PLANS.md unless a deeper instruction asks for one. Because the copied
spec is itself Markdown, do not wrap the whole file in a fenced code block.

A good spec is self-contained, living, novice-readable, and focused on a
demonstrably working result. A future agent or maintainer should be able to
restart from only the current working tree and this file.
-->

# <Short, action-oriented title>

- Branch: <branch-name>
- Status: Draft | In Progress | Ready for Review | Merged
- Owner(s): <name(s)>
- Created: <YYYY-MM-DD>
- Last Updated: <YYYY-MM-DD>
- Links: [Issue](url) | [PR](url) | [Design/Doc](url)

This ExecPlan is a living document. Keep `Progress`, `Surprises & Discoveries`,
`Decision Log`, and `Outcomes & Retrospective` current as research,
implementation, validation, and review proceed.
When the next milestone is clear, continue to it and update the spec instead of
asking for generic next steps.

## Purpose / Big Picture

Explain in a few plain-language paragraphs what someone gains after this
change. Describe the user-visible or operator-visible behavior this branch will
enable, how someone can see it working, and why the work matters.

State non-goals explicitly. If the change is internal, explain the external
effect it protects or improves and how that effect can be demonstrated.

Success means:

- <Observable behavior or metric that proves success>
- <Command, UI flow, API response, eval, or log signal that confirms it>
- <Boundary that remains intentionally out of scope>

## Progress

Update this list at every stopping point. Use timestamps so another reader can
tell what is done, what remains, and when the state changed. Split partially
completed work into "completed" and "remaining" notes instead of leaving it
ambiguous.

- [ ] (<YYYY-MM-DD HH:MMZ>) Research current behavior and relevant code paths.
- [ ] (<YYYY-MM-DD HH:MMZ>) Define the implementation approach and validation plan.
- [ ] (<YYYY-MM-DD HH:MMZ>) Implement milestone 1.
- [ ] (<YYYY-MM-DD HH:MMZ>) Implement milestone 2.
- [ ] (<YYYY-MM-DD HH:MMZ>) Run validation and record evidence.
- [ ] (<YYYY-MM-DD HH:MMZ>) Update PR/spec with final outcome and residual risks.

## Surprises & Discoveries

Record unexpected behavior, library constraints, failing assumptions,
performance findings, migration issues, or review discoveries that shape the
work. Include concise evidence, such as a test name, terminal excerpt, source
path, or log line.

- Observation: <What was learned>
  Evidence: <Command, file path, output excerpt, or review link>

## Decision Log

Record every meaningful design or workflow decision. Include why this choice
was made so a future contributor does not have to reconstruct prior discussion.

- Decision: <Chosen path>
  Rationale: <Why this path fits the repo, user goal, constraints, or risk>
  Date/Author: <YYYY-MM-DD / name or agent>

## Outcomes & Retrospective

At major milestones and at completion, summarize what changed, what now works,
what remains, and what lessons should carry forward. Compare the result against
`Purpose / Big Picture`.

- Outcome: <What was achieved or learned>
  Evidence: <Validation, PR, commit, screenshot, eval, or logs>
  Remaining: <Gaps, follow-up tickets, or "None">

## Context and Orientation

Write this section for a reader who knows nothing beyond the current checkout
and this file. Define any term of art in plain language when you first use it.
Do not rely on prior chat, memory, external articles, or unstated assumptions.

Relevant repo rules:

- Read the applicable `AGENTS.md` chain before changing files.
- Use `lib/db/queries/*.ts` for database operations; do not import `db`
  directly.
- Follow Effect-first implementation patterns and the Effect docs listed in
  `AGENTS.md` when touching TypeScript application code.
- Use `@effect/vitest` with `it.effect` for new tests unless explicitly
  approved otherwise.
- Prefer `tsgo` over `tsc` for type checks.
- Run vitest with `--run`, for example `bun run test --run <file>`.

Current behavior:

- <Describe the existing behavior, bug, missing capability, or workflow>
- <Name the full repository-relative files, modules, routes, functions, jobs,
  tables, env vars, flags, and tests that matter>

How the relevant pieces fit together:

- <Short orientation paragraph that connects routes, services, database,
  workers, UI, AI/tool orchestration, or external services>

Assumptions:

- <Environment, feature flag, deployment, data, account, or dependency
  assumption>
- <Fallback if the assumption is not true>

## Plan of Work

Describe the implementation in prose. For each milestone, state what will exist
after the milestone that did not exist before, the files to edit, the commands
to run, and the acceptance signal to observe. Each milestone should be
independently verifiable and should move the branch toward a working behavior.

### Milestone 1: <Name>

Scope: <What this milestone changes and why it is first>

Files and interfaces:

- `<repo-relative/path.ts>`: <Function, component, schema, service, query, route,
  or type to add/change>
- `<repo-relative/test.ts>`: <Test or eval coverage to add/change>

Work:

Describe the concrete edits in enough detail that a novice can make or review
them. Prefer stable names and paths. If a new interface, function, schema,
table, route, environment variable, or Effect service must exist, specify its
name, inputs, outputs, and failure modes.

Acceptance:

- Run `<command from repo root>` and expect `<observable result>`.
- Exercise `<UI/API/worker/eval scenario>` and expect `<observable result>`.

### Milestone 2: <Name>

Scope: <What this milestone changes and why it follows milestone 1>

Files and interfaces:

- `<repo-relative/path.ts>`: <Function, component, schema, service, query, route,
  or type to add/change>
- `<repo-relative/test.ts>`: <Test or eval coverage to add/change>

Work:

<Concrete edits and constraints>

Acceptance:

- Run `<command from repo root>` and expect `<observable result>`.
- Exercise `<UI/API/worker/eval scenario>` and expect `<observable result>`.

## Interfaces and Dependencies

Be prescriptive about the local and external APIs the implementation will rely
on. For third-party libraries, inspect installed package metadata, lockfiles,
source, and tests before depending on behavior. Capture the relevant API shape
here so future implementers do not rely on stale or unversioned knowledge.

Local interfaces:

- `<module.function or type>`:
  - Inputs: <types and constraints>
  - Outputs: <types and success shape>
  - Failures: <typed errors, Effect causes, thrown defects, HTTP statuses, or
    retry behavior>

External dependencies:

- `<package/service>`:
  - Version/source checked: <package version, lockfile, opensrc path, or source
    URL>
  - Expected behavior: <inputs, outputs, lifecycle, edge cases, and constraints>
  - Failure handling: <what the branch should do when it fails>

## Concrete Steps

List exact commands with the working directory. Update this section as work
proceeds so the commands reflect reality, not just the initial guess. Include
short expected output excerpts where they help distinguish success from failure.

From the branch checkout's repository root (`<repo root>`):

    rg "<search term>" <path>
    bun run test --run <test-file>
    bun run <typecheck-or-build-command>

Expected evidence:

    <short terminal excerpt, HTTP response, eval result, migration output, or
    log line proving the behavior>

## Validation and Acceptance

Validation is required. Acceptance must be behavior a human can observe, not
only "code was added" or "types compile." Include the exact tests, type checks,
manual flows, evals, migrations, and runtime probes appropriate for the branch.

Automated validation:

- `<command>`: expect `<pass count, relevant test name, or output>`
- `<command>`: expect `<typecheck/build/eval result>`

Manual or runtime validation:

- Start: `<command>` from `<working directory>`
- Exercise: `<URL, API call, UI flow, worker trigger, or CLI invocation>`
- Observe: `<specific response, screen state, log, metric, or artifact>`

Regression checks:

- <Existing behavior that must continue to work>
- <How to verify it still works>

## Idempotence and Recovery

Explain how to repeat steps safely. If migrations, backfills, generated files,
external side effects, or destructive operations are involved, state the safe
retry path, cleanup path, and backout path.

- Re-running `<command>` is safe because <reason>.
- If `<step>` fails halfway, recover by <commands or manual steps>.
- Backout plan: <feature flag, revert strategy, migration rollback, data repair,
  or deployment rollback>.

## Rollout and Operations

Describe how this reaches users or operators and how to monitor it.

- Feature flags/config/env vars: <names, defaults, deployment requirements>
- Migration/backfill steps: <files, order, safety notes>
- Monitoring/alerts/logs: <signals to watch and expected healthy values>
- PR/branch workflow: <commit, push, PR, review, or deploy expectations>

## Risks and Open Questions

Keep this current. For each risk, include a mitigation or the validation that
will reduce uncertainty. Resolve open questions in the `Decision Log` when they
become decisions.

- Risk: <What could go wrong>
  Mitigation: <How this spec addresses it>
- Open question: <What remains unknown>
  Owner/next step: <Who or what resolves it>

## Artifacts and Notes

Include only concise evidence that helps prove or reconstruct the work:
important transcripts, small diffs, screenshots, eval summaries, dashboard
links, PR review links, or source excerpts. Keep large logs and generated
artifacts out of the spec unless they are essential.

    <Indented evidence excerpt>

## Revision Notes

When revising this spec, add a note describing what changed and why. Keep the
whole document internally consistent after each revision.

- <YYYY-MM-DD>: <Change and reason>