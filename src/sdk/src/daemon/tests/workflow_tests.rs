//! Workflow dispatch integration with daemon-wide resource limits.

use std::sync::Arc;

use crate::daemon::task_loop::workflow::RuntimeDispatch;
use crate::flow_engine::caps::dispatch::HarnessDispatch;
use crate::hub::TaskRequest;

use super::{base_config, blocking_runner, recording_send};

#[tokio::test]
async fn workflow_dispatch_waits_for_a_daemon_harness_slot() {
    let (ready_tx, mut ready_rx) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(tokio::sync::Notify::new());
    let mut config = base_config();
    config.concurrency = 1;
    let (send, _) = recording_send();
    let runtime =
        crate::daemon::DaemonRuntime::new(config, blocking_runner(ready_tx, gate.clone()), send);
    let occupied = runtime
        .inner
        .slots
        .acquire()
        .await
        .expect("semaphore stays open");

    let dispatch = RuntimeDispatch::new(runtime.clone(), "peer".into());
    let task = tokio::spawn(async move {
        dispatch
            .dispatch(TaskRequest {
                task_id: "review-1".into(),
                abort_id: "run-1".into(),
                cycle_id: None,
                instruction: "review the failed workflow".into(),
                worker_address: "claude".into(),
                provider: None,
                custom_harness: None,
                model: None,
                tool_mode: Some("propose:sweep".into()),
                workflow: None,
                conversation: None,
            })
            .await
    });

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), ready_rx.recv())
            .await
            .is_err(),
        "the review must not start while the only slot is occupied"
    );
    drop(occupied);
    tokio::time::timeout(std::time::Duration::from_secs(1), ready_rx.recv())
        .await
        .expect("dispatch starts after the slot is released")
        .expect("runner reports readiness");
    gate.notify_waiters();
    task.await
        .expect("dispatch task joins")
        .expect("dispatch runs");
}
