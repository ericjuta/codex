use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use super::RuntimeCommand;
use super::RuntimeState;
use super::value::value_to_error_text;

pub(super) struct ScheduledTimeout {
    callback: v8::Global<v8::Function>,
}

pub(super) struct TimerScheduler {
    shared: Arc<TimerShared>,
    worker: Option<thread::JoinHandle<()>>,
}

struct TimerShared {
    state: Mutex<TimerState>,
    wake: Condvar,
    runtime_command_tx: std::sync::mpsc::Sender<RuntimeCommand>,
}

#[derive(Default)]
struct TimerState {
    deadlines: HashMap<u64, Instant>,
    heap: BinaryHeap<Reverse<(Instant, u64)>>,
    shutdown: bool,
}

impl TimerScheduler {
    pub(super) fn new(runtime_command_tx: std::sync::mpsc::Sender<RuntimeCommand>) -> Self {
        let shared = Arc::new(TimerShared {
            state: Mutex::new(TimerState::default()),
            wake: Condvar::new(),
            runtime_command_tx,
        });
        Self {
            shared,
            worker: None,
        }
    }

    pub(super) fn schedule(&mut self, id: u64, delay_ms: u64) -> Result<(), String> {
        if self.worker.is_none() {
            let worker_shared = Arc::clone(&self.shared);
            self.worker = Some(
                thread::Builder::new()
                    .name("codex-code-mode-timer".to_string())
                    .spawn(move || run_timer_scheduler(worker_shared))
                    .map_err(|error| format!("failed to spawn code-mode timer thread: {error}"))?,
            );
        }
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(delay_ms))
            .unwrap_or_else(Instant::now);
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.deadlines.insert(id, deadline);
        state.heap.push(Reverse((deadline, id)));
        self.shared.wake.notify_one();
        Ok(())
    }

    pub(super) fn clear(&self, id: u64) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.deadlines.remove(&id);
        self.shared.wake.notify_one();
    }
}

impl Drop for TimerScheduler {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.shutdown = true;
        }
        self.shared.wake.notify_one();
        let _ = worker.join();
    }
}

fn run_timer_scheduler(shared: Arc<TimerShared>) {
    loop {
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if state.shutdown {
                return;
            }
            let Some(Reverse((deadline, timeout_id))) = state.heap.peek().copied() else {
                state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            };
            if state.deadlines.get(&timeout_id) != Some(&deadline) {
                state.heap.pop();
                continue;
            }
            let now = Instant::now();
            if deadline <= now {
                state.heap.pop();
                state.deadlines.remove(&timeout_id);
                drop(state);
                let _ = shared
                    .runtime_command_tx
                    .send(RuntimeCommand::TimeoutFired { id: timeout_id });
                break;
            }
            let wait_duration = deadline.saturating_duration_since(now);
            let wait_result = shared
                .wake
                .wait_timeout(state, wait_duration)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = wait_result.0;
        }
    }
}

pub(super) fn schedule_timeout(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
) -> Result<u64, String> {
    let callback = args.get(0);
    if !callback.is_function() {
        return Err("setTimeout expects a function callback".to_string());
    }
    let callback = v8::Local::<v8::Function>::try_from(callback)
        .map_err(|_| "setTimeout expects a function callback".to_string())?;

    let delay_ms = args
        .get(1)
        .number_value(scope)
        .map(normalize_delay_ms)
        .unwrap_or(0);

    let callback = v8::Global::new(scope, callback);
    let state = scope
        .get_slot_mut::<RuntimeState>()
        .ok_or_else(|| "runtime state unavailable".to_string())?;
    let timeout_id = state.next_timeout_id;
    state.next_timeout_id = state.next_timeout_id.saturating_add(1);
    state.timer_scheduler.schedule(timeout_id, delay_ms)?;
    state
        .pending_timeouts
        .insert(timeout_id, ScheduledTimeout { callback });

    Ok(timeout_id)
}

pub(super) fn clear_timeout(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
) -> Result<(), String> {
    let Some(timeout_id) = timeout_id_from_args(scope, args)? else {
        return Ok(());
    };

    let Some(state) = scope.get_slot_mut::<RuntimeState>() else {
        return Err("runtime state unavailable".to_string());
    };
    if state.pending_timeouts.remove(&timeout_id).is_some() {
        state.timer_scheduler.clear(timeout_id);
    }
    Ok(())
}

pub(super) fn invoke_timeout_callback(
    scope: &mut v8::PinScope<'_, '_>,
    timeout_id: u64,
) -> Result<(), String> {
    let callback = {
        let state = scope
            .get_slot_mut::<RuntimeState>()
            .ok_or_else(|| "runtime state unavailable".to_string())?;
        state.pending_timeouts.remove(&timeout_id)
    };
    let Some(callback) = callback else {
        return Ok(());
    };

    let tc = std::pin::pin!(v8::TryCatch::new(scope));
    let mut tc = tc.init();
    let callback = v8::Local::new(&tc, &callback.callback);
    let receiver = v8::undefined(&tc).into();
    let _ = callback.call(&tc, receiver, &[]);
    if tc.has_caught() {
        return Err(tc
            .exception()
            .map(|exception| value_to_error_text(&mut tc, exception))
            .unwrap_or_else(|| "unknown code mode exception".to_string()));
    }

    Ok(())
}
fn timeout_id_from_args(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
) -> Result<Option<u64>, String> {
    if args.length() == 0 || args.get(0).is_null_or_undefined() {
        return Ok(None);
    }

    let Some(timeout_id) = args.get(0).number_value(scope) else {
        return Err("clearTimeout expects a numeric timeout id".to_string());
    };
    if !timeout_id.is_finite() || timeout_id <= 0.0 {
        return Ok(None);
    }

    Ok(Some(timeout_id.trunc().min(u64::MAX as f64) as u64))
}

fn normalize_delay_ms(delay_ms: f64) -> u64 {
    if !delay_ms.is_finite() || delay_ms <= 0.0 {
        0
    } else {
        delay_ms.trunc().min(u64::MAX as f64) as u64
    }
}

#[cfg(test)]
#[path = "timers_tests.rs"]
mod tests;
