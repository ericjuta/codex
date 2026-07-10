use crate::agent::AgentControl;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::protocol::MultiAgentVersion;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::Barrier;

fn control_with_limit(max_threads: usize) -> AgentControl {
    let control = AgentControl::default();
    control.agent_execution_limiter.initialize(max_threads);
    control
}

fn subagent_source() -> SessionSource {
    SessionSource::SubAgent(SubAgentSource::Other("worker".to_string()))
}

fn assert_agent_limit(err: CodexErr, expected_max_threads: usize) {
    let CodexErr::AgentLimitReached { max_threads } = err else {
        panic!("expected AgentLimitReached");
    };
    assert_eq!(max_threads, expected_max_threads);
}

#[test]
fn execution_reservations_transfer_to_running_turns_and_roll_back() {
    let control = control_with_limit(/*max_threads*/ 1);
    // Child role configs cannot replace the root-derived session limit.
    control
        .agent_execution_limiter
        .initialize(/*max_threads*/ 2);
    let source = subagent_source();
    let first_thread_id = ThreadId::new();
    let second_thread_id = ThreadId::new();

    let first_admission = Arc::clone(&control.agent_execution_limiter)
        .reserve_thread(first_thread_id)
        .expect("first pending turn should fit");
    let err = match Arc::clone(&control.agent_execution_limiter).reserve_thread(second_thread_id) {
        Ok(_) => panic!("second pending turn should exceed the derived non-root cap"),
        Err(err) => err,
    };
    assert_agent_limit(err, /*expected_max_threads*/ 1);

    first_admission.commit();
    let first_guard = control
        .execution_guard(first_thread_id, MultiAgentVersion::V2, &source)
        .expect("pending admission should transfer")
        .expect("v2 subagent execution should be counted");
    let err = match Arc::clone(&control.agent_execution_limiter).reserve_thread(second_thread_id) {
        Ok(_) => panic!("running turn should continue holding capacity"),
        Err(err) => err,
    };
    assert_agent_limit(err, /*expected_max_threads*/ 1);

    drop(first_guard);
    let rolled_back = Arc::clone(&control.agent_execution_limiter)
        .reserve_thread(second_thread_id)
        .expect("released capacity should admit another turn");
    drop(rolled_back);
    let retry = Arc::clone(&control.agent_execution_limiter)
        .reserve_thread(second_thread_id)
        .expect("failed submission should roll its reservation back");
    retry.commit();
    control.release_execution_reservation(second_thread_id);
}

#[test]
fn concurrent_execution_reservations_admit_only_capacity() {
    const CONTENDERS: usize = 8;
    let control = control_with_limit(/*max_threads*/ 1);
    let barrier = Arc::new(Barrier::new(CONTENDERS));
    let limiter = Arc::clone(&control.agent_execution_limiter);

    let results = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(CONTENDERS);
        for _ in 0..CONTENDERS {
            let barrier = Arc::clone(&barrier);
            let limiter = Arc::clone(&limiter);
            handles.push(scope.spawn(move || {
                let thread_id = ThreadId::new();
                barrier.wait();
                let result = Arc::clone(&limiter).reserve_thread(thread_id);
                (thread_id, result)
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("reservation thread should not panic"))
            .collect::<Vec<_>>()
    });

    let mut admitted = Vec::new();
    let mut rejected = 0;
    for (thread_id, result) in results {
        match result {
            Ok(admission) => admitted.push((thread_id, admission)),
            Err(err) => {
                assert_agent_limit(err, /*expected_max_threads*/ 1);
                rejected += 1;
            }
        }
    }
    assert_eq!(admitted.len(), 1);
    assert_eq!(rejected, CONTENDERS - 1);

    let (thread_id, admission) = admitted.pop().expect("one reservation should win");
    admission.commit();
    let guard = Arc::clone(&limiter)
        .take_or_try_guard(thread_id)
        .expect("winning reservation should transfer");
    drop(guard);
}

#[test]
fn execution_guards_bind_spawned_threads_and_ignore_root_and_v1_turns() {
    let control = control_with_limit(/*max_threads*/ 1);
    let source = subagent_source();
    let thread_id = ThreadId::new();

    let guard = control
        .reserve_execution_guard(MultiAgentVersion::V2, &source)
        .expect("spawn reservation should fit")
        .expect("v2 subagent spawn should reserve capacity");
    control.bind_execution_guard(thread_id, guard);
    let running = control
        .execution_guard(thread_id, MultiAgentVersion::V2, &source)
        .expect("bound spawn reservation should transfer")
        .expect("v2 subagent execution should be counted");
    drop(running);

    assert!(
        control
            .execution_guard(ThreadId::new(), MultiAgentVersion::V2, &SessionSource::Cli,)
            .expect("root turns are unlimited")
            .is_none()
    );
    assert!(
        control
            .execution_guard(ThreadId::new(), MultiAgentVersion::V1, &source)
            .expect("v1 turns use the legacy limit")
            .is_none()
    );
}
