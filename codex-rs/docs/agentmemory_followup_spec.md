# Agentmemory Follow-Up Spec

## Status

Proposed follow-up backlog after the runtime-surface, proactive-guidance, and
visual-memory-UI lanes.

This document exists to answer one practical question:

- what is still worth doing after the current `agentmemory` integration work,
- in what order,
- and what should explicitly not be done yet.

## Current State

The fork already has:

- `agentmemory` as the active long-term memory backend
- an assistant-facing `memory_recall` tool
- human-facing `/memory-recall`, `/memory-update`, and `/memory-drop`
- proactive runtime guidance for when the assistant should use recall
- dedicated visual memory history cells in both TUIs

That means the system is functionally good. What remains is mostly structural
cleanup, richer human visibility, and better retrieval/capture quality.

## Goal

Turn the current good private-fork integration into the cleanest durable shape:

- fewer duplicated UI heuristics
- clearer human visibility for all memory activity
- more structured memory events and metadata
- better long-term retrieval quality
- better end-to-end confidence

## Priority Order

### 1. Replace string-matched memory UI with structured memory events

Current state:

- both TUIs recognize memory outcomes by parsing core-emitted warning/error
  strings

Why this is next:

- it removes duplicated parsing logic across `tui` and `tui_app_server`
- it makes the visual memory UI more robust against copy changes
- it creates the right hook point for surfacing assistant-triggered memory use

Target shape:

- a dedicated protocol event carrying:
  - operation
  - status
  - query
  - whether context was injected
  - preview/detail payload

Do not do:

- broad protocol redesign beyond the memory event itself

### 2. Replace append-only pending memory cards with in-place completion updates

Current state:

- the human sees a `Pending` card at submit time and then a second final card

Why this is next:

- the current UX is correct but noisy
- users can misread the persistent pending card as a stuck operation

Target shape:

- one memory card per operation
- pending transitions to ready/empty/error in place

### 3. Surface assistant-triggered `memory_recall` to the human

Current state:

- human-triggered memory actions are visually shown
- assistant-triggered `memory_recall` is functionally real but not given the
  same polished human-facing transcript treatment

Why this matters:

- users should be able to see when the assistant consulted long-term memory
- this improves trust and debuggability

Target shape:

- assistant-triggered memory recall produces the same visual memory event style
  as human-triggered recall
- the UI should distinguish:
  - tool returned context to the assistant
  - context was injected into the current thread

### 4. Add richer memory metadata to the human UI

Current state:

- memory cells show operation/query/status/preview
- they do not yet show richer recall metadata

Useful additions:

- block count
- token count
- backend/source label
- timestamp or relative freshness label

Why this matters:

- it helps users understand whether memory was broad, sparse, or stale

### 5. Add a lightweight memory-availability indicator in the TUI

Current state:

- memory is visible when explicitly used
- there is no ambient signal that the current runtime has `agentmemory` recall
  available

Target shape:

- a subtle status-line or bottom-pane indication when:
  - backend is `agentmemory`
  - `memory_recall` tool is available

Do not do:

- a large always-on panel

### 6. Add end-to-end regression coverage for assistant memory use

Current state:

- focused tool/spec/TUI tests exist
- there is no single end-to-end regression proving the assistant actually calls
  `memory_recall` in a realistic run and that the human can observe the right
  result path

Target additions:

- a `codex exec`-style regression for assistant tool recall
- a TUI/app-server regression for visual memory event rendering

### 7. Finish the payload-quality backlog

This stays important even though the runtime surface is now solid.

Still open from the payload-quality spec:

- add tool-output size caps for `post_tool_use`
- selectively filter low-value `pre_tool_use` traffic
- create real-session quality evaluation fixtures

Why this still matters:

- retrieval quality will eventually matter more than UI polish

### 8. Consider selective auto-recall only after the above is done

Current state:

- recall is explicit and targeted
- assistant guidance is now better

This is intentionally not earlier in the order because:

- auto-recall before structured events and better observability is harder to
  trust
- over-eager recall can create noise, token waste, and hard-to-debug behavior

If done later, it should be narrow:

- only when current-thread context is obviously insufficient
- only with targeted queries
- only after the human can clearly see that memory was consulted

## Non-Goals

Do not do these in the next lane unless requirements change:

- MCP-based memory exposure
- a second competing memory backend
- broad automatic recall on every turn
- large static memory dumps into the prompt
- major UI chrome like a full separate memory sidebar

## Acceptance Gates For The Next Meaningful Lane

The next follow-up lane should count as complete only if:

- memory events are structured rather than inferred from strings
- the human sees a single coherent memory card per operation
- assistant-triggered recall is visible to the human
- the UI still stays aligned between `tui` and `tui_app_server`

## Recommendation

If choosing only one next lane, do this:

- implement structured memory protocol events and use them to replace the
  current string-parsing visual UI path

That is the best leverage point because it improves:

- UI clarity
- assistant transparency
- maintainability
- long-term extensibility

If choosing two lanes, do these in order:

1. structured memory events
2. payload-quality backlog (`post_tool_use` caps + `pre_tool_use` filtering)
