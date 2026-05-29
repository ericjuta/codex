# Code Mode Operational Hardening Spec

This spec defines the next code-mode hardening pass for daily WWYDN-style
operation: make nested-tool execution observable, prove cancellation and output
drain behavior under stress, and burn in multi-agent fanout without changing the
user-facing code-mode contract.

The intent is not to redesign code mode. The intent is to make the existing
runtime safer to operate: when code-mode cells spawn nested tools, wait on
subagents, terminate work, or yield output, operators should have counters,
tests, and burn-in scripts that explain what happened.

## Goals

- Add low-cardinality telemetry for code-mode cells and nested tool calls so
  operators can distinguish runtime startup, nested-tool dispatch, cancellation,
  output drain, and fanout pressure.
- Prove that cell termination cancels in-flight nested tools and does not leave
  stale workers, sessions, or pending turn messages behind.
- Prove that output drain behavior is bounded and observable for both completed
  and terminated cells.
- Burn in timer-heavy JavaScript and fast large-output commands so local
  runtime overhead can be separated from model or provider latency.
- Keep config knobs out of scope until measured runtime data points to a
  specific tuning need.
- Burn in multi-agent fanout from code mode, especially `spawn_agent`,
  `followup_task`, `wait_agent`, and mailbox wake behavior.
- Keep the prompt-facing and runtime-facing code-mode tool contracts unchanged
  unless a later implementation spec explicitly changes them.

## Non-Goals

- Do not change model-visible tool names, schemas, or prompt descriptions as part
  of this pass.
- Do not change the top-level multi-agent scheduling contract or mailbox wake
  semantics from this spec alone.
- Do not add product documentation or user-facing docs.
- Do not add host-specific configuration, private paths, tokens, or raw logs to
  committed artifacts.
- Do not add new runtime configuration before the burn-in identifies a concrete
  local bottleneck.
- Do not use high-cardinality telemetry labels such as cell ids, call ids,
  task names, file paths, prompts, or free-form error strings.

## Existing Surfaces

- Code-mode session and cell orchestration lives in
  `codex-rs/code-mode/src/service.rs`.
  The service owns session handles, cancellation tokens, worker routes, pending
  turn messages, nested tool invocation, notification delivery, and cell
  termination.
- The V8 runtime boundary lives in `codex-rs/code-mode/src/runtime/`.
  Runtime outcomes already distinguish completed execution from pending nested
  tool work and expose output-drain-oriented wait outcomes.
- Host-side turn execution and in-flight tool draining live in
  `codex-rs/core/src/session/turn.rs`.
  The turn loop drains in-flight tool futures before returning and has the
  current mailbox preemption hook for multi-agent v2.
- Tool prompt/runtime assembly lives around `codex-rs/core/src/tools/spec_plan.rs`
  and the code-mode tool specs under `codex-rs/core/src/tools/code_mode/`.
  This pass should not collapse the existing split between runtime registry and
  prompt-facing descriptions.
- Session telemetry infrastructure already exists in `codex-rs/core/src/session/`
  and related telemetry helpers. Code-mode metrics should reuse that style when
  the code-mode service has access to the relevant recorder.

## Telemetry Plan

Add counters and durations with bounded labels. The exact metric API can follow
the nearest existing `SessionTelemetry` helpers, but the label set should stay
small enough for routine production use.

Suggested counters:

| Metric | Labels | Counted when |
| --- | --- | --- |
| `codex.code_mode.cell_started` | `source` | A cell starts through `exec` or `wait` |
| `codex.code_mode.cell_completed` | `source`, `status` | A cell completes, errors, yields, or is terminated |
| `codex.code_mode.nested_tool_started` | `tool`, `source` | A nested tool invocation is dispatched to the host |
| `codex.code_mode.nested_tool_completed` | `tool`, `status` | A nested tool returns, errors, or is cancelled |
| `codex.code_mode.output_drained` | `source`, `status` | Output drain finishes, times out, or is skipped |
| `codex.code_mode.worker_route_closed` | `reason` | A worker route is closed or replaced |
| `codex.code_mode.pending_turn_messages` | `operation`, `status` | Pending turn messages are queued, delivered, or dropped |
| `codex.code_mode.timer_callback` | `status` | A JavaScript timer callback is scheduled, run, or cancelled |

Suggested duration metrics:

| Metric | Labels | Measured interval |
| --- | --- | --- |
| `codex.code_mode.cell_duration` | `source`, `status` | Cell start to terminal or yielded outcome |
| `codex.code_mode.nested_tool_duration` | `tool`, `status` | Host nested-tool invocation duration |
| `codex.code_mode.output_drain_duration` | `source`, `status` | Runtime output drain attempt duration |
| `codex.code_mode.output_bytes` | `tool`, `status` | Bytes returned through a nested tool result |
| `codex.code_mode.timer_delay` | `status` | Requested JavaScript timer delay |

Allowed label values:

- `source`: `exec`, `wait`, `runtime`, `terminate`, `worker`.
- `status`: `ok`, `error`, `cancelled`, `yielded`, `timeout`, `dropped`.
- `tool`: stable nested tool names only, such as `shell_command`,
  `apply_patch`, `spawn_agent`, `wait_agent`, and `followup_task`.
- `operation`: `queue`, `deliver`, `drop`.
- `reason`: `completed`, `cancelled`, `replaced`, `session_closed`.

Do not label metrics by cell id, call id, branch, cwd, prompt text, task name,
agent id, error message, or output content. If a debugging workflow needs those
values, keep them in structured traces or local debug logs, not in metric
cardinality.

## Runtime Burn-In Matrix

The burn-in should keep model and provider latency visible but separate from
local runtime overhead. A first matrix should include:

| Scenario | Required proof |
| --- | --- |
| Parallel nested shell tools | All nested calls complete and report bounded nested-tool durations |
| Yield and later wait | The later wait routes to the original cell worker or a queued pending route |
| Terminate with nested shell running | The child process exits and cancellation metrics increment |
| Timer-heavy cell | Timer callbacks complete without per-timer OS thread growth |
| Fast large output | Output bytes and post-exit drain duration are recorded |
| Repeated turns | Worker routes and pending messages return to zero at cleanup |
| Multi-agent fanout | Spawned agents complete or close with no stale live agents |

The summary should report turn count, cell count, nested tool count, cancelled
nested tool count, output bytes, p50 and p95 nested-tool duration, p50 and p95
output-drain duration, max pending-message depth, max rss, and elapsed time.

This matrix can start as a deterministic local script. It should avoid external
network dependencies and should clean up temporary files, child processes, and
spawned agents before exiting.

## Cancellation And Drain Proof

The hardening pass should extend focused tests before changing behavior.

Required test coverage:

- Terminating a cell with an in-flight nested tool cancels the tool and releases
  the session handle.
- Terminating a cell while runtime output is pending drains or drops output by an
  explicit bounded rule and records the terminal status.
- A nested tool that observes cancellation cannot later deliver a successful
  result into the terminated cell.
- Repeated execute/terminate cycles do not accumulate live workers, pending turn
  messages, or active sessions.
- Wait on an already terminated or unknown cell returns a stable error without
  reviving worker state.
- Timer-heavy cells complete without creating one OS thread per timer callback.
- Fast large-output nested shell commands record output size and bounded
  post-exit drain timing.

Preferred proof shape:

1. Add or extend `codex-rs/code-mode/src/service.rs` tests using fake hosts that
   can block, observe cancellation, and report whether output was delivered.
2. Add runtime-level tests only when the service tests cannot see the relevant
   V8 boundary.
3. Keep timeouts short and explicit in tests so cancellation failures become
   clear hangs rather than slow flakes.

## Multi-Agent Fanout Burn-In

The burn-in should exercise code-mode nested tool fanout without relying on
private host state.

Minimum scenario:

1. Start one code-mode cell that spawns multiple focused subagents.
2. Use a mix of passive mailbox checks and waking follow-ups:
   `wait_agent`, `send_message`, and `followup_task`.
3. Terminate at least one still-active branch of work.
4. Verify that the root turn receives completed agent output, queued mailbox
   state is visible, and no stale live agents remain after cleanup.
5. Repeat the scenario enough times to catch worker-route reuse and pending
   message ordering bugs.

Observable output should include:

- Number of cells started and completed.
- Number of nested tool invocations by tool name.
- Number of subagents spawned, completed, cancelled, and closed.
- Wait outcomes split into completed, timed out, and mailbox-only.
- Any pending turn messages left at the end of the scenario.

This burn-in can begin as a focused test harness or local script. If it becomes a
committed script, keep it deterministic, avoid external network dependencies,
and make the cleanup path close all spawned agents.

## Implementation Order

1. Add instrumentation seams in the code-mode service with no behavior change.
2. Add focused service tests for cancellation, output drain, and stale-state
   cleanup.
3. Add nested-tool and cell metrics behind the existing telemetry path.
4. Add the multi-agent fanout burn-in harness.
5. Run the focused code-mode test slice and then the relevant `codex-core` slice
   if the implementation touches core turn draining or tool specs.
6. Only after proof exists, consider behavior fixes for any burn-in failures.

## Acceptance Criteria

- Code-mode metrics distinguish cell lifecycle, nested-tool lifecycle,
  cancellation, output drain, and pending-message handling without high-cardinality
  labels.
- Termination of a cell with in-flight nested tools is covered by tests that
  prove cancellation reaches the host and stale output cannot revive the cell.
- Output drain has focused tests for completed and terminated cells.
- Multi-agent fanout has a repeatable burn-in path that records outcomes and
  cleans up spawned agents.
- Timer-heavy and fast large-output cases are part of the burn-in summary.
- No new config knob is proposed without burn-in evidence naming the local
  bottleneck it addresses.
- Prompt-facing code-mode tool documentation and runtime nested-tool registry are
  unchanged unless a separate spec and tests justify a contract change.
- The final implementation runs the repo-required formatter from `codex-rs` and
  the focused test package, with any broader `just test` run requested before
  execution if common or core-wide behavior changes require it.
