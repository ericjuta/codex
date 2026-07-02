# Code Mode Review Remediation Spec

This spec turns the findings of the 2026-07-02 multi-agent review of the
code-mode feature (main @ `7864f814e1`, 42 raw findings, 17 adversarially
confirmed) into concrete remediation workstreams. The review asked three
questions — is the feature sound, effective, and performant for daily-driven
codex sessions — and concluded: the cell runtime itself is well-engineered,
but core never reclaims cells it abandons, the dispatch layer serializes the
feature's own concurrency design, and a handful of ergonomic and backpressure
defects turn routine events (interrupt, model switch, response bursts) into
lost work.

The intent is not to redesign code mode. Each workstream below fixes a
confirmed defect at its root, with the smallest contract change that removes
the failure mode. Workstreams are ordered by leverage: WS1 alone resolves four
confirmed findings that share one root cause.

## Goals

- Make Esc/turn-interrupt actually stop code-mode work: no orphaned cells, no
  nested tool calls executing in later turns, no leaked host resources.
- Let the dispatch layer exploit the concurrency the cell runtime already
  provides (yielded cells + wait) instead of serializing it behind the
  turn-wide tool lock.
- Guarantee that a completed cell's output is never irrecoverably lost to a
  cancellation race.
- Turn IPC overload into backpressure instead of connection death.
- Remove the model-facing footguns that waste turns: string rejections,
  silently-ignored parameters, misleading "not found" errors, catchable
  `exit()`.
- Surface config mistakes (missing host binary, contradictory namespace
  flags, per-thread flag drift) at startup or config load, not per-exec.
- Bound runtime resource growth over a long session (timers, stored values,
  abandoned isolates).

## Non-Goals

- No redesign of the three-crate split (`code-mode`, `code-mode-host`,
  `code-mode-protocol`) or the actor model — the review confirmed both are
  sound.
- No changes to model-visible tool names or the exec/wait lifecycle contract,
  except the narrow additions listed in WS5 (error shapes, parameter alias,
  one doc line).
- Telemetry counters and burn-in harnesses remain governed by
  `code_mode_operational_hardening_spec.md`; WS9 only reconciles that spec
  with the shipped architecture.
- No new user-facing configuration knobs unless a workstream explicitly
  requires one.

## WS1 — Cell reclamation on interrupt, abort, and mode flip (P0)

Root cause shared by four confirmed findings: the only paths that terminate a
host cell are the model's explicit `wait { terminate: true }`
(`codex-rs/core/src/tools/code_mode/wait_handler.rs:82-87`) and full session
shutdown (`codex-rs/core/src/session/handlers.rs:598`). Every other way a
cell can be abandoned leaks it.

Resolves:

- [high] Turn interrupt never propagates to an executing cell; its nested
  tool calls queue in the session-lived unbounded dispatch channel
  (`codex-rs/code-mode/src/service.rs` broker, `delegate.rs:35`) and execute
  under the NEXT turn's worker
  (`codex-rs/core/src/tools/code_mode/execute_handler.rs:71`).
- [medium] Abandoned cells permanently pin one of the 128 host-global
  active-cell permits (`codex-rs/code-mode-host/src/lib.rs:42`); after 128,
  every exec in every thread fails until codex restarts.
- [medium] Each abandoned cell keeps an OS thread + V8 isolate (~3-10MB)
  alive until session shutdown.
- [medium] A mid-session model switch that flips `effective_tool_mode` away
  from CodeMode starts no dispatch worker and unregisters exec/wait, so live
  background cells strand their nested calls and notifications forever
  (`codex-rs/core/src/tools/code_mode/mod.rs:143`).

Changes:

1. `CodeModeExecuteHandler` must observe `invocation.cancellation_token`.
   On cancellation during the yield window, call `CodeModeService::terminate`
   for the started cell before returning the aborted response. Do not rely on
   the 1s-grace-then-abort in
   `codex-rs/core/src/tools/parallel.rs:161-176` to clean up — task abort
   drops the future without running termination.
2. `CodeModeWaitHandler` must likewise terminate-on-cancel when the turn is
   interrupted while waiting, OR explicitly hand the cell back to the
   session's live-cell set (see item 3) — waiting cells whose scripts are
   intentionally backgrounded must survive Esc, so termination here applies
   only to cells the turn itself started and never yielded to the model.
3. `CodeModeService` keeps a per-session registry of live (yielded) cell IDs.
   At turn end, cells that yielded and were surfaced to the model (their
   cell ID appeared in a tool output) stay alive; cells that were started but
   whose ID never reached the model are terminated — the model cannot ever
   wait on them, so keeping them is a pure leak.
4. On a turn whose `effective_tool_mode` is not CodeMode/CodeModeOnly while
   live cells exist: terminate all live cells and emit a developer-visible
   notice ("N running code cells were terminated because the active model
   does not support code mode"). This is strictly better than the current
   silent strand; preserving cells across a mode flip is out of scope.
5. Nested tool dispatch must stop honoring invocations queued by a previous
   turn: tag queued `DispatchMessage::InvokeTool` entries with the turn they
   were submitted in, and have the worker drop (with an error response to the
   host) entries from dead turns instead of executing them under the new
   turn's context.

Tests (currently zero interrupt/abort coverage in
`codex-rs/core/tests/suite/code_mode.rs`):

- Interrupt during exec yield window → cell terminated on host, permit
  released, no nested tool executes after the abort.
- Interrupt while a nested shell command is in flight → nested command's
  cancellation token fires.
- Yielded background cell + Esc on a later turn → cell survives (it is
  model-visible), but a never-yielded cell from the interrupted turn is
  reaped.
- Model switch with a live background cell → cell terminated, notice emitted,
  dispatch queue empty.
- Permit-leak regression: loop exec-then-interrupt 130 times on one host; the
  131st exec must succeed.

## WS2 — Dispatch-layer parallelism (P1)

Resolves: [medium] exec and wait take the turn-wide write lock
(`supports_parallel_tool_calls` defaults to false,
`codex-rs/tools/src/tool_executor.rs:64-66`; taken at
`codex-rs/core/src/tools/parallel.rs:123`), and exec holds it through the
entire yield window — up to `DEFAULT_EXEC_YIELD_TIME_MS` = 10s
(`codex-rs/code-mode-protocol/src/runtime.rs:11`). Waiting on 3 background
cells costs 30s instead of 10s; the concurrent-cells design is defeated at
the dispatch layer.

Changes:

1. `CodeModeWaitHandler` overrides `supports_parallel_tool_calls() -> true`.
   Wait is a pure observation; it mutates nothing in core.
2. Exec is restructured so the turn write lock is not held across the yield
   window. Two acceptable shapes, in order of preference:
   - Exec also declares `supports_parallel_tool_calls() -> true` and instead
     serializes its *nested* tool dispatches through the existing
     `ToolCallRuntime` locking (each nested call already goes through real
     tool dispatch, which takes the appropriate lock per nested tool). The
     outer exec call then only coordinates, and mutual exclusion lives where
     the mutation actually happens.
   - If audit shows a nested-tool path that bypasses `ToolCallRuntime`
     locking, keep exec exclusive but split dispatch: take the write lock
     only until the cell is started, then release before awaiting
     `started_cell.initial_response()`.
3. Add a wall-clock regression test: two `wait` calls on two already-yielded
   cells issued in one model response complete in ~max(t1, t2), not t1+t2.

## WS3 — Completion delivery durability (P1)

Resolves: [medium] a cell's final result is permanently lost when a wait is
cancelled concurrently with completion delivery
(`codex-rs/code-mode/src/cell_actor/types.rs:247`). `send(Ok(event))`
returning Ok is treated as durable delivery and the cell is tombstoned, but
the receiver can be dropped without reading the value — host-side via the
biased cancellation select in `codex-rs/code-mode-host/src/lib.rs:358-368`,
client-side via `responses.rs:213` sending into an abandoned oneshot. The
model's next wait then gets `WaitOutcome::MissingCell` ("exec cell N not
found", `codex-rs/code-mode/src/service.rs:401-407`) and the output is
irrecoverable. This violates the exact invariant the buffered-completion
machinery (CellPhase::Completed / CompletionClaimed) exists to provide.

Changes:

1. Delivery is durable only when acknowledged, not when sent. Replace the
   fire-and-forget oneshot send with an explicit claim/ack protocol: the
   observer acks receipt (or the request task completes its response write);
   until then the completion stays buffered and a dropped/cancelled observer
   rebuffers it — extending the behavior the existing tests
   (`observation_dropped_before_dequeue_does_not_consume_output`,
   `failed_completion_delivery_rebuffers_the_event`) already pin for the
   receiver-already-dropped case to the sent-but-never-read case.
2. The Wait request path gains the abandoned-result handling the Execute path
   already has (`codex-rs/code-mode/src/remote_session/connection/driver/responses.rs:156-165`
   `terminate_abandoned_cell`): when a WaitCompleted response arrives for a
   request core has abandoned, rebuffer or explicitly terminate — never
   silently drop.
3. Distinguish terminal states in wait errors: "cell N already completed and
   was closed" vs "cell N not found", and stop rendering the former under a
   "Script failed" header. The model must be able to tell a double-wait from
   a genuinely unknown cell.
4. Test: fire CancelRequest and completion delivery in the same poll window
   (loom-style or forced-ordering test at the actor level) and assert the
   next wait returns the buffered output.

## WS4 — IPC backpressure instead of connection death (P1)

Resolves: [medium] both sides of the IPC path convert a momentarily full
128-slot queue into connection teardown: client
`driver.queue_frame` → `TrySendError::Full` → `self.fail(...)` drains all
sessions (`codex-rs/code-mode/src/remote_session/connection/driver.rs:136-138`);
host `HostPeer::send_frame` → Full → `self.disconnect()`
(`codex-rs/code-mode-host/src/peer.rs:393-400`). A `Promise.all` over ~200
nested calls (delegate permit cap is 256) or concurrent image-emitting cells
fills the queue while a multi-MB frame (`MAX_FRAME_BYTES` = 64MB,
`codec.rs:12`) is being written, and every session sharing the host dies.

Changes:

1. Replace `try_send`-then-kill with awaited `send()` on both sides. Senders
   are async tasks that can tolerate waiting; the queue bound then functions
   as backpressure, which is the reason to have a bound at all.
2. Where an await is impossible (synchronous contexts, the reader loop that
   must not deadlock against its own writer), spill to an unbounded local
   buffer with a high-water warning rather than failing the connection.
   Audit for reader/writer cycles first: if the reader must queue a frame
   whose consumer is blocked sending to the reader, awaiting introduces a
   deadlock — the spill path is mandatory there.
3. Connection failure remains the response to genuine peer death (EOF, codec
   error, oversized frame) only.
4. Test: a cell fanning out 200+ nested calls whose responses complete
   simultaneously, plus a slow-reader harness, completes with elevated
   latency and zero disconnects.

## WS5 — Model ergonomics bundle (P2)

Resolves the confirmed turn-wasting footguns identified by the ergonomics
assessment:

1. Nested tool failures must reject with real `Error` objects, not strings
   (`codex-rs/code-mode/src/runtime/module_loader.rs:88-92`). Idiomatic
   `catch (e) { text(e.message) }` currently prints "undefined"; the repo's
   own test works around it with `e?.message ?? String(e)`. Construct a
   `v8::Exception::error` (or an `Error` subclass carrying the tool name)
   so `.message`, `.stack`, and `instanceof Error` behave.
2. Unify the wait/exec token-limit parameter: exec takes
   `max_output_tokens`, wait takes `max_tokens`, and unknown fields are
   silently ignored. Add a serde alias so both names work on both tools, and
   enable `deny_unknown_fields` on both argument structs so a genuinely
   wrong parameter errors instead of silently defaulting.
3. Make `exit()` uncatchable: route it through the runtime command channel
   (as termination already is) instead of a thrown string sentinel that
   defensive `try/catch` swallows.
4. Add one line to the exec tool description stating that output reaches the
   model only via `text()` / `image()` / `notify()` and that the script's
   completion value is not itself surfaced — the review found models
   plausibly assuming otherwise.

Contract note: items 1-3 change runtime-visible behavior inside cells; land
them together behind a single changelog entry so prompt regressions (models
relying on string rejections) are bisectable to one commit.

## WS6 — Config and startup diagnostics (P2)

Resolves three confirmed config-flag findings:

1. [medium] `code_mode_only` + `features.code_mode.excluded_tool_namespaces`
   silently deletes the excluded tools from BOTH surfaces:
   `is_hidden_by_code_mode_only`
   (`codex-rs/core/src/tools/spec_plan.rs:460-471`) hides them from the
   direct surface without consulting `is_excluded_from_code_mode`, while
   `build_code_mode_executors` (`spec_plan.rs:506-508`) omits them from the
   nested surface. Fix: under `code_mode_only`, a namespace excluded from
   the nested surface stays a direct tool (mirror the
   `direct_only_tool_namespaces` override path,
   `apply_direct_model_only_namespace_overrides`, `spec_plan.rs:205-232`).
2. [low] `features.code_mode_host` with no `codex-code-mode-host` binary next
   to the executable (`codex-rs/code-mode/src/remote_session.rs:497`) is
   accepted silently and fails per-exec. Fix: check
   `CODEX_CODE_MODE_HOST_PATH` / the sibling path at session-provider
   construction; on missing binary, log a startup diagnostic naming the
   expected path and fall back to the in-process runtime rather than
   advertising a tool surface every call of which fails.
3. [low] `Feature::CodeModeHost` is read once at `ThreadManager::new`
   (`codex-rs/core/src/thread_manager.rs:345-349`) and the same provider Arc
   serves every thread regardless of per-thread config overrides
   (`thread_manager.rs:1586`). Either resolve the provider per-thread from
   the thread's own config, or — if the shared-host design is intentional —
   log a warning when a thread's config disagrees with the manager-level
   provider so the drift is visible instead of silent.

## WS7 — Runtime resource hygiene (P2)

Resolves the confirmed slow-burn performance findings inside the runtime:

1. Timer wheel (`codex-rs/code-mode/src/runtime/timers.rs:39`): `setTimeout`
   currently spawns a dedicated OS thread per timer and `clearTimeout` leaves
   it sleeping for the full delay. Replace with a single sleeper thread per
   runtime (binary heap of deadlines, condvar wakeup). This is also the one
   outright contradiction of the operational hardening spec's resource-growth
   constraint, so it unblocks WS9.
2. Async isolate creation
   (`codex-rs/code-mode/src/runtime/mod.rs:119`,
   `codex-rs/code-mode/src/session_runtime/mod.rs:167`): `start_cell` blocks
   a tokio worker on a std `mpsc::recv` waiting for thread spawn + `v8::Isolate::new`
   while holding the session-wide `cells` mutex, and first-exec V8
   platform/ICU init runs synchronously on the same worker. Move isolate
   spawn outside the `cells` lock and await it via a tokio oneshot (or
   `spawn_blocking`); run `initialize_v8()` once at provider startup instead
   of lazily on the first cell's hot path.
3. Stored-values growth
   (`codex-rs/code-mode/src/session_runtime/mod.rs:162, 295-297`): the whole
   map is deep-cloned into every new cell and re-serialized JSON→string→V8
   per exec, grows monotonically, and is never pruned. Share entries as
   `Arc<str>` of pre-serialized JSON (clone becomes refcount bumps; V8 parse
   cost remains but the copy and re-serialization disappear), and lazily
   materialize into the isolate on first `load()` of each key rather than
   injecting the full map up front. A size cap with an explicit
   eviction/error is acceptable as a follow-up; silent eviction is not.

## WS8 — Token economics in mixed CodeMode (P2)

Resolves: [low] in `ToolMode::CodeMode` (code_mode on, code_mode_only off)
every request pays for each classic tool's full JSON schema PLUS a TS
declaration block re-rendering the identical schema
(`codex-rs/core/src/tools/spec_plan.rs:282`,
`codex-rs/tools/src/code_mode.rs:8-51`,
`codex-rs/code-mode-protocol/src/description.rs:372-384, 660-667`), plus
~1,070 tokens of exec/wait specs — roughly 2,500-6,000 extra input tokens per
request for a 10-20 tool session.

Changes:

1. In mixed CodeMode, stop appending the full TS declaration to each classic
   tool's description. Replace with a single short pointer in the exec spec
   ("all direct tools are also callable as `tools.<name>(...)` inside cells;
   signatures match their JSON schemas") plus the output type only — the
   output type is the one datum the JSON schema does not carry
   (`#[serde(skip)]` on `ResponsesApiTool`).
2. Keep CodeModeOnly rendering unchanged (the TS block is the sole schema
   surface there and is correctly paid once).
3. Refresh the committed snapshot in `codex-rs/core/tests/suite/code_mode.rs`
   and record the measured per-request token delta in the PR description so
   the win is quantified.

## WS9 — Observability and spec reconciliation (P3)

The hardening spec's stated prerequisites for daily operation (low-cardinality
counters, cancellation burn-in, output-drain proofs) are largely
unimplemented, and its "Existing Surfaces" section describes pre-rebase
architecture. After WS1-WS4 land:

1. Introduce a structured error enum at the `CodeModeSession` boundary
   (`codex-rs/code-mode/src/service.rs`) replacing stringly errors — this is
   the prerequisite for the spec's counter taxonomy and for WS3 item 3's
   distinct terminal states.
2. Implement the spec's cell-lifecycle counters (started, yielded,
   terminated, reaped-by-WS1, completion-rebuffered-by-WS3, permits-in-use)
   — WS1 and WS3 create exactly the events worth counting.
3. Add a notification-drain watchdog: `finish_callbacks` with
   `CallbackCompletion::DrainNotifications`
   (`codex-rs/code-mode/src/cell_actor/mod.rs:443`) currently awaits
   notification tasks with no timeout while the actor mailbox is blocked, so
   a stalled `notify()` wedges the cell and every queued wait forever. Bound
   the drain (spec suggests 5s) and count timeouts.
4. Amend `code_mode_operational_hardening_spec.md`'s "Existing Surfaces" to
   match the shipped process-host architecture, and mark its timer constraint
   as satisfied once WS7 item 1 lands.

## Sequencing and finding coverage

| Order | Workstream | Priority | Confirmed findings resolved |
| --- | --- | --- | --- |
| 1 | WS1 cell reclamation | P0 | high interrupt-orphan; medium permit leak; medium isolate leak; medium mode-flip strand (x2, same root) |
| 2 | WS2 dispatch parallelism | P1 | medium turn-lock serialization |
| 3 | WS3 completion durability | P1 | medium wait-cancel result loss |
| 4 | WS4 IPC backpressure | P1 | medium try_send connection death |
| 5 | WS5 ergonomics bundle | P2 | error shapes, param mismatch, catchable exit (assessment findings) |
| 6 | WS6 config diagnostics | P2 | medium excluded-namespace deletion; low missing host binary; low per-thread provider drift |
| 7 | WS7 runtime hygiene | P2 | low timer threads; low blocking isolate spawn (x2); low stored-values growth |
| 8 | WS8 token economics | P2 | low mixed-mode double-pay |
| 9 | WS9 observability | P3 | low notification-drain wedge; hardening-spec gap |

WS1 is the gate for daily-driver soundness; WS2 is the largest interactive
latency win; WS3/WS4 remove the remaining lost-work modes. Everything after
is quality-of-life and can land opportunistically.
