//! Workflow dispatch integration with daemon-wide resource limits.

use std::sync::Arc;

use crate::daemon::providers::{RunTaskFn, RunTaskResult};
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
                workflow_fingerprint: None,
                workflow_inputs: Default::default(),
                conversation: None,
                fleet_depth: 0,
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

#[tokio::test]
async fn workflow_dispatch_preserves_the_callers_fleet_depth() {
    let (captured_tx, mut captured_rx) = tokio::sync::mpsc::unbounded_channel();
    let runner: RunTaskFn = Arc::new(move |options| {
        let captured_tx = captured_tx.clone();
        Box::pin(async move {
            captured_tx.send(options.env.clone()).unwrap();
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: options.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    });
    let (send, _) = recording_send();
    let runtime = crate::daemon::DaemonRuntime::new(base_config(), runner, send);
    let dispatch = RuntimeDispatch::new(runtime, "peer".into());

    dispatch
        .dispatch(TaskRequest {
            task_id: "child-1".into(),
            abort_id: "child-1".into(),
            cycle_id: None,
            instruction: "do the child work".into(),
            worker_address: "claude".into(),
            provider: None,
            custom_harness: None,
            model: None,
            tool_mode: Some("execute".into()),
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 2,
        })
        .await
        .unwrap();

    let env = captured_rx.recv().await.unwrap();
    assert_eq!(
        env.get(crate::control_socket::FLEET_DEPTH_ENV)
            .map(String::as_str),
        Some("2")
    );
}
