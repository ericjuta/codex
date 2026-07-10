use super::AgentControl;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::protocol::MultiAgentVersion;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SessionSource;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;

#[derive(Default)]
pub(super) struct AgentExecutionLimiter {
    state: Mutex<AgentExecutionState>,
    max_threads: OnceLock<usize>,
}

#[derive(Default)]
struct AgentExecutionState {
    active: usize,
    next_reservation_id: u64,
    pending: HashMap<ThreadId, PendingExecution>,
}

struct PendingExecution {
    reservation_id: u64,
    admissions: usize,
    accepted: bool,
}

pub(crate) struct AgentExecutionAdmission {
    limiter: Arc<AgentExecutionLimiter>,
    thread_id: ThreadId,
    reservation_id: u64,
    active: bool,
}

pub(crate) struct AgentExecutionGuard {
    limiter: Arc<AgentExecutionLimiter>,
    active: bool,
}

impl AgentExecutionAdmission {
    pub(crate) fn commit(mut self) {
        self.limiter
            .finish_admission(self.thread_id, self.reservation_id, true);
        self.active = false;
    }
}

impl Drop for AgentExecutionAdmission {
    fn drop(&mut self) {
        if self.active {
            self.limiter
                .finish_admission(self.thread_id, self.reservation_id, false);
        }
    }
}

impl Drop for AgentExecutionGuard {
    fn drop(&mut self) {
        if self.active {
            self.limiter.release_guard();
        }
    }
}

impl AgentControl {
    pub(crate) async fn reserve_execution_capacity_for_op(
        &self,
        thread_id: ThreadId,
        op: &Op,
    ) -> CodexResult<Option<AgentExecutionAdmission>> {
        self.reserve_execution_capacity_for_turn_start(thread_id, op_starts_turn(op))
            .await
    }

    pub(super) async fn reserve_execution_capacity_for_turn_start(
        &self,
        thread_id: ThreadId,
        starts_turn: bool,
    ) -> CodexResult<Option<AgentExecutionAdmission>> {
        if !starts_turn {
            return Ok(None);
        }
        let state = self.upgrade()?;
        let thread = state.get_thread(thread_id).await?;
        let config = thread.codex.session.get_config().await;
        let multi_agent_version = thread
            .multi_agent_version()
            .unwrap_or_else(|| config.multi_agent_version_from_features());
        if !is_execution_limited(multi_agent_version, &thread.session_source) {
            return Ok(None);
        }
        let active_turn = thread.codex.session.active_turn.lock().await;
        if active_turn.is_some() {
            return Ok(None);
        }
        Arc::clone(&self.agent_execution_limiter)
            .reserve_thread(thread_id)
            .map(Some)
    }

    pub(crate) fn reserve_execution_guard(
        &self,
        multi_agent_version: MultiAgentVersion,
        session_source: &SessionSource,
    ) -> CodexResult<Option<AgentExecutionGuard>> {
        if !is_execution_limited(multi_agent_version, session_source) {
            return Ok(None);
        }
        Arc::clone(&self.agent_execution_limiter)
            .try_guard()
            .map(Some)
    }

    pub(crate) fn bind_execution_guard(&self, thread_id: ThreadId, guard: AgentExecutionGuard) {
        self.agent_execution_limiter.bind_guard(thread_id, guard);
    }

    pub(crate) fn execution_guard(
        &self,
        thread_id: ThreadId,
        multi_agent_version: MultiAgentVersion,
        session_source: &SessionSource,
    ) -> CodexResult<Option<AgentExecutionGuard>> {
        if !is_execution_limited(multi_agent_version, session_source) {
            return Ok(None);
        }
        Arc::clone(&self.agent_execution_limiter)
            .take_or_try_guard(thread_id)
            .map(Some)
    }

    pub(crate) fn release_execution_reservation(&self, thread_id: ThreadId) {
        self.agent_execution_limiter.release_thread(thread_id);
    }
}

impl AgentExecutionLimiter {
    pub(super) fn initialize(&self, max_threads: usize) {
        self.max_threads.get_or_init(|| max_threads);
    }

    fn max_threads(&self) -> usize {
        self.max_threads.get().copied().unwrap_or(usize::MAX)
    }

    fn reserve_thread(
        self: Arc<Self>,
        thread_id: ThreadId,
    ) -> CodexResult<AgentExecutionAdmission> {
        let mut state = self.lock_state();
        let reservation_id = if let Some(pending) = state.pending.get_mut(&thread_id) {
            pending.admissions += 1;
            pending.reservation_id
        } else {
            if state.active >= self.max_threads() {
                return Err(CodexErr::AgentLimitReached {
                    max_threads: self.max_threads(),
                });
            }
            state.active += 1;
            state.next_reservation_id = state.next_reservation_id.wrapping_add(1);
            let reservation_id = state.next_reservation_id;
            state.pending.insert(
                thread_id,
                PendingExecution {
                    reservation_id,
                    admissions: 1,
                    accepted: false,
                },
            );
            reservation_id
        };
        drop(state);
        Ok(AgentExecutionAdmission {
            limiter: self,
            thread_id,
            reservation_id,
            active: true,
        })
    }

    fn bind_guard(&self, thread_id: ThreadId, mut guard: AgentExecutionGuard) {
        debug_assert!(std::ptr::eq(self, guard.limiter.as_ref()));
        let mut state = self.lock_state();
        if state.pending.contains_key(&thread_id) {
            drop(state);
            return;
        }
        state.next_reservation_id = state.next_reservation_id.wrapping_add(1);
        let reservation_id = state.next_reservation_id;
        state.pending.insert(
            thread_id,
            PendingExecution {
                reservation_id,
                admissions: 0,
                accepted: true,
            },
        );
        guard.active = false;
    }

    fn try_guard(self: Arc<Self>) -> CodexResult<AgentExecutionGuard> {
        let mut state = self.lock_state();
        if state.active >= self.max_threads() {
            return Err(CodexErr::AgentLimitReached {
                max_threads: self.max_threads(),
            });
        }
        state.active += 1;
        drop(state);
        Ok(AgentExecutionGuard {
            limiter: self,
            active: true,
        })
    }

    fn take_or_try_guard(self: Arc<Self>, thread_id: ThreadId) -> CodexResult<AgentExecutionGuard> {
        let mut state = self.lock_state();
        if state.pending.remove(&thread_id).is_none() {
            if state.active >= self.max_threads() {
                return Err(CodexErr::AgentLimitReached {
                    max_threads: self.max_threads(),
                });
            }
            state.active += 1;
        }
        drop(state);
        Ok(AgentExecutionGuard {
            limiter: self,
            active: true,
        })
    }

    fn finish_admission(&self, thread_id: ThreadId, reservation_id: u64, accepted: bool) {
        let mut state = self.lock_state();
        let should_release = if let Some(pending) = state.pending.get_mut(&thread_id)
            && pending.reservation_id == reservation_id
        {
            pending.accepted |= accepted;
            pending.admissions = pending.admissions.saturating_sub(1);
            pending.admissions == 0 && !pending.accepted
        } else {
            false
        };
        if should_release {
            state.pending.remove(&thread_id);
            state.active = state.active.saturating_sub(1);
        }
    }

    fn release_thread(&self, thread_id: ThreadId) {
        let mut state = self.lock_state();
        if state.pending.remove(&thread_id).is_some() {
            state.active = state.active.saturating_sub(1);
        }
    }

    fn release_guard(&self) {
        let mut state = self.lock_state();
        debug_assert!(state.active > 0);
        state.active = state.active.saturating_sub(1);
    }

    fn lock_state(&self) -> MutexGuard<'_, AgentExecutionState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn op_starts_turn(op: &Op) -> bool {
    matches!(op, Op::UserInput { .. })
        || matches!(op, Op::InterAgentCommunication { communication } if communication.trigger_turn)
}

fn is_execution_limited(
    multi_agent_version: MultiAgentVersion,
    session_source: &SessionSource,
) -> bool {
    multi_agent_version == MultiAgentVersion::V2
        && matches!(session_source, SessionSource::SubAgent(_))
}

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;
