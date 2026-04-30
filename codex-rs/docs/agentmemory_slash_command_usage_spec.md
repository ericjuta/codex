# Agentmemory Slash Command Usage Spec

## Status

Implemented documentation reference for the current `codex` runtime surface.

This document is not a backend architecture spec.

It exists to answer a narrower operational question:

- when should a human operator use each `agentmemory` slash command in Codex?

## Goal

Make the human-facing `agentmemory` slash surfaces easy to use correctly
without requiring the operator to infer intent from tool names alone.

The runtime already supports these commands.

This spec defines the intended operator workflow for each one.

## Default Rule

Do not reach for a memory slash command on every turn.

Normal Codex work should continue to rely on:

- automatic `agentmemory` integration
- normal conversation context
- packet-backed resume behavior

Use explicit slash commands only when you need one of:

1. targeted recovery of missing context
2. explicit durable saving
3. visibility into coordination state
4. visibility into failure-avoidance guidance
5. visibility into synthesized project/file knowledge

## Command Guidance

### `/memory-recall [query]`

Use when:

- you believe relevant past context exists but is not currently in thread
- you are resuming after a gap
- you want prior rationale, prior bug history, or prior file history

Good examples:

- “why did we change this file before?”
- “what happened in the last attempt on this feature?”
- “bring back the prior design rationale for auth refresh”

Do not use when:

- the current thread already contains the needed context
- you only want the next action rather than past context

### `/memory-remember <content>`

Use when:

- you want to explicitly save durable knowledge
- the information is important enough to survive this thread

Good examples:

- stable repo-specific workflow rules
- failure shields that should be remembered later
- explicit durable facts or design decisions

Do not use for:

- temporary TODOs
- normal task recap
- information that is obviously ephemeral

### `/memory-missions`

Use when:

- you want the durable objective container
- you need to understand project-level goal, owner, phase, or blockers

Good examples:

- “what mission is active?”
- “what objective is currently blocked?”
- “what higher-level thing are these actions serving?”

### `/memory-handoffs`

Use when:

- you want the durable resume artifact itself
- you need a human-readable handoff summary
- you are handing work to another agent or person

Supported usage patterns:

- `/memory-handoffs`
  - review recent packets for the current project
- `/memory-handoffs <handoff_packet_id>`
  - fetch one specific packet
- `/memory-handoffs session`
  - review the latest session-scoped packet for the current thread
- `/memory-handoffs mission <mission_id>`
  - review mission-scoped packets
- `/memory-handoffs action <action_id>`
  - review action-scoped packets

Preferred operator behavior:

- use `/memory-handoffs session` when you want the packet-backed “where am I?”
  answer for the current thread

### `/memory-handoff-generate [session|mission|action] <id>`

Use when:

- you are about to stop
- you are about to switch threads
- you want to create a fresh handoff packet on demand

Good examples:

- end-of-day handoff
- before delegating to another agent
- before asking someone else to resume implementation

### `/memory-actions`

Use when:

- you want the concrete work items
- you care about actionable task state more than objective state

Good examples:

- “show blocked actions”
- “what explicit tasks are tracked?”

### `/memory-frontier`

Use when:

- you want the current set of unblocked options
- you are choosing among several next moves

Use instead of `/memory-next` when:

- you want a menu of choices rather than one recommendation

### `/memory-next`

Use when:

- you want one recommendation only
- you are unsure what to do next after completing a step

### `/memory-guardrails`

Use when:

- you want failure-avoidance guidance
- you are entering a risky edit path
- you suspect there are known traps in this area

Good examples:

- before changing sensitive infra or build flows
- after noticing repeated regressions
- before touching files with history of costly mistakes

### `/memory-decisions`

Use when:

- you need the recorded tradeoff and rationale
- you want to understand active assumptions
- you want to know when a decision should be revisited

Good examples:

- “why did we choose this architecture?”
- “what decision is currently governing this lane?”

### `/memory-dossiers`

Use when:

- you need a file-level orientation brief
- you are opening a hotspot file after time away
- you want current state, risks, and open questions for one component

Good examples:

- before editing a large, high-churn file
- when onboarding yourself back into a subsystem

### `/memory-branch-overlays`

Use when:

- branch-local context matters
- you do not want local branch context confused with project-global truth

Good examples:

- worktree-specific blockers
- branch-only notes before merge
- temporary branch-local coordination state

### `/memory-routine-candidates`

Use when:

- you want to inspect repeated successful patterns
- you are thinking about process formalization rather than immediate execution

Good examples:

- repeated release or verification flows
- repeated debugging sequences

Do not use as your default next-step command.

### `/memory-lessons`

Use when:

- you want compact learned takeaways
- you want prior learned guidance without the heavier structure of decisions

### `/memory-insights`

Use when:

- you want higher-order synthesized patterns
- you want broader conclusions rather than narrow lessons

### `/memory-crystals`

Use when:

- you want compressed action-chain summaries
- you want a distilled recap of a completed execution arc

## Operator Decision Table

- Missing past context: use `/memory-recall`
- Save durable knowledge explicitly: use `/memory-remember`
- Check objective-level state: use `/memory-missions`
- Check packet-backed handoff state: use `/memory-handoffs`
- Check task-level work items: use `/memory-actions`
- Get one recommended next move: use `/memory-next`
- Get several unblocked options: use `/memory-frontier`
- Check failure shields: use `/memory-guardrails`
- Check durable rationale: use `/memory-decisions`
- Get oriented on one file: use `/memory-dossiers`
- Inspect branch-only context: use `/memory-branch-overlays`
- Review repeated successful patterns: use `/memory-routine-candidates`

## UX Notes

The current Codex runtime should preserve these principles:

- slash commands remain optional, not mandatory
- auto memory behavior should handle the common case
- explicit commands should be used when the operator wants targeted control
- handoff commands should remain packet-first for resume workflows

## Acceptance Criteria

This usage spec is complete when:

- each human-facing `agentmemory` slash command has a defined intended use
- resume-oriented handoff usage is explicit
- “don’t use this every turn” guidance is explicit
- operators can choose between recall, missions, actions, handoffs, guardrails,
  decisions, and dossiers without guessing
