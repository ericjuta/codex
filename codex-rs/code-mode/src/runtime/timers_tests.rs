use std::sync::mpsc;
use std::time::Duration;

use super::TimerScheduler;
use crate::runtime::RuntimeCommand;

#[test]
fn timer_worker_starts_lazily_and_is_reused() {
    let (command_tx, command_rx) = mpsc::channel();
    let mut scheduler = TimerScheduler::new(command_tx);
    assert!(scheduler.worker.is_none());

    scheduler.schedule(/*id*/ 1, /*delay_ms*/ 0).unwrap();
    let worker_id = scheduler
        .worker
        .as_ref()
        .expect("timer worker should start on first schedule")
        .thread()
        .id();
    assert!(matches!(
        command_rx.recv_timeout(Duration::from_secs(1)),
        Ok(RuntimeCommand::TimeoutFired { id: 1 })
    ));

    scheduler.schedule(/*id*/ 2, /*delay_ms*/ 0).unwrap();
    assert_eq!(
        scheduler
            .worker
            .as_ref()
            .expect("timer worker should remain available")
            .thread()
            .id(),
        worker_id
    );
    assert!(matches!(
        command_rx.recv_timeout(Duration::from_secs(1)),
        Ok(RuntimeCommand::TimeoutFired { id: 2 })
    ));
}
