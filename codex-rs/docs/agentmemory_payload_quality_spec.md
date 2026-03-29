# Agentmemory Payload Quality Spec

## Goal

Improve the usefulness of Agentmemory-derived observations, timelines, and
retrieval context for Codex sessions.

The current integration is functionally working, but memory quality is still
limited by:

- lifecycle-heavy noise dominating the observation stream
- incomplete structured tool payloads
- missing assistant-result capture
- weaker file-aware enrichment than the standalone Agentmemory hook scripts

## Current State

What is already working:

- Codex sessions are registered in Agentmemory and appear in the viewer.
- Session lifecycle is closed out on Codex shutdown.
- Hook payloads now use Agentmemory-compatible hook names:
  - `session_start`
  - `prompt_submit`
  - `pre_tool_use`
  - `post_tool_use`
  - `post_tool_failure`
  - `assistant_result`
  - `stop`
- Prompt, tool input, tool output, and error fields are mapped into the
  canonical Agentmemory schema.
- Structured tool arguments are parsed from JSON command strings into
  searchable top-level fields (`file_path`, `path`, `pattern`, `query`, etc.).
- File-aware enrichment surfaces `files[]` and `search_terms[]` on tool events.
- Assistant conclusions are captured at turn completion (`is_final: true`) and
  progressively as each message block completes streaming (`is_final: false`).
- Mid-session memory retrieval via `/memory-recall [query]` injects recalled
  context as developer messages into the active conversation.
- Token-budgeted context injection at session startup via `/agentmemory/context`.
- All event capture is non-blocking via `tokio::spawn`.
- Assistant text truncated to 4096 bytes respecting UTF-8 boundaries.

Remaining gaps:

- Tool output payloads (`post_tool_use`) have no size cap and may cause
  memory bloat for large file reads.
- `pre_tool_use` fires unconditionally for all tools; no selective filtering
  to reduce timeline noise for low-signal events.
- Real-session quality evaluation fixtures are deferred (unit tests exist).

## Desired Outcomes

1. Agentmemory timelines should be dominated by user intent, important tool
results, failures, decisions, and conclusions instead of routine lifecycle
markers.
2. Retrieval context should help a later agent answer:
   - what the user asked
   - what the agent tried
   - what succeeded or failed
   - what conclusion or decision mattered
3. File-sensitive tasks should yield observations and memories that mention the
   relevant paths and search terms when available.
4. Manual memory recall should be inspectable by the human in the TUI, not only
   injected into assistant context.

## Proposed Changes

### 1. Reduce lifecycle noise

Default policy:

- Keep:
  - `prompt_submit`
  - `post_tool_use`
  - `post_tool_failure`
  - `session_start`
  - `stop`
- Suppress or aggressively gate:
  - `pre_tool_use`

Preferred rule:

- Do not emit `pre_tool_use` for routine shell or exec traffic by default.
- Only emit `pre_tool_use` when it carries unique high-signal metadata that
  will not appear in the corresponding post-tool observation.

Acceptance criteria:

- A typical session timeline should contain substantially fewer lifecycle-only
  observations.
- Repeated pre-tool lifecycle entries should no longer dominate the top of the
  timeline for normal sessions.

### 2. Preserve structured tool arguments where available

For tool-use events, prefer structured payloads over flattened command strings
when the source event includes them.

Examples of desired fields:

- file-oriented tools:
  - `file_path`
  - `path`
  - `paths`
  - `pattern`
- search-oriented tools:
  - `query`
  - `pattern`
  - `glob`
- execution tools:
  - structured command arguments when available

If both structured fields and a command string exist:

- preserve the structured fields in `tool_input`
- optionally keep the command string under a separate field if it adds value

Acceptance criteria:

- Agentmemory compressed observations for file/search/edit tasks should more
  often include exact file paths and more task-specific titles.

### 3. Capture assistant result / conclusion payloads

Add a new observation path for assistant-visible conclusions, not only tool
activity.

Possible event classes:

- final assistant message at turn completion
- synthesized task result / conclusion
- important stop-summary payload when the agent has a meaningful last answer

Minimum useful fields:

- assistant text, truncated to a safe size
- turn id
- session id
- cwd
- optional tags for whether the text is final, partial, or summary content

Acceptance criteria:

- Sessions with little or no tool usage still produce useful memory records.
- Retrieval can surface what the agent concluded, not just what tools ran.

### 4. Improve file-aware enrichment parity

Bring the Rust integration closer to the standalone Agentmemory JavaScript hook
behavior for file-aware tools.

When structured file/search arguments are available, enable the same sort of
file-context enrichment that the JavaScript `pre-tool-use` hook performs.

Acceptance criteria:

- Memory observations for file edits/searches are more likely to mention the
  touched paths and relevant search terms.

### 5. Add quality evaluation fixtures

Create a small regression corpus of real Codex sessions and evaluate:

- timeline readability
- compressed observation usefulness
- retrieval usefulness for follow-up questions

Suggested evaluation checks:

1. Timeline signal ratio
   - proportion of useful task observations vs lifecycle-only observations
2. Retrieval usefulness
   - given a follow-up question, does returned context contain the task, action,
     result, and conclusion?
3. File recall quality
   - for file-sensitive sessions, do observations and retrieval mention the
     correct paths?

Acceptance criteria:

- At least one representative multi-tool session becomes obviously more useful
  to inspect in the viewer after the changes.
- Retrieval answers improve on a fixed before/after comparison for the same
  session set.

## Non-Goals

- perfect semantic summarization of every session
- preserving every lifecycle marker in the durable memory stream
- storing unbounded tool outputs
- introducing heavy blocking calls on the hot path of Codex tool execution

## Rollout Order

1. ~~suppress or gate low-value lifecycle observations~~ — kept all events including pre_tool_use; enriched with structured args instead of gating
2. ~~forward richer structured tool input~~ — implemented
3. ~~add assistant-result capture~~ — implemented; streaming intermediate capture added (`is_final: false` per completed message block)
4. ~~add evaluation fixtures and compare before/after quality~~ — unit tests added; real-session fixture comparison is deferred
5. ~~mid-session memory retrieval~~ — implemented via `/memory-recall [query]` slash command and `Op::RecallMemories`
6. tool output size caps — not yet implemented
7. selective pre_tool_use filtering — not yet implemented

## Risks

- over-filtering may remove useful debugging evidence
- assistant-result capture may duplicate information already present in tool
  outputs if not scoped carefully
- richer structured payloads may increase observation size and compression cost

## Mitigations

- keep raw observation size limits and truncation
- prefer targeted gating over blanket event deletion
- evaluate with real-session fixtures before expanding the payload surface
