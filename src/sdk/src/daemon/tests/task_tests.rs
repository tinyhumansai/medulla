//! Task-lifecycle tests: acceptance limits, duplicate rejection, stdin/input
//! forwarding (including buffering before the sink registers), and shutdown
//! aborting an in-flight task.

use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{mpsc, Notify};

use crate::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use crate::daemon::DaemonRuntime;
use crate::tinyplace::TaskFrameKind;

use super::{
    abortable_runner, base_config, blocking_runner, decoded_frames, input_frame, recording_send,
    stdin_runner, task_frame, wait_ready,
};

#[tokio::test]
async fn rejects_tasks_over_max_pending() {
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let run_task = blocking_runner(ready_tx, gate.clone());
    let (send, recorded) = recording_send();
    let mut config = base_config();
    config.concurrency = 1;
    config.max_pending = 1;
    let runtime = DaemonRuntime::new(config, run_task, send);

    // Task A occupies the single pending slot and blocks.
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    wait_ready(&mut ready_rx).await;

    // Task B is rejected at capacity.
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t2", "more", None)),
    );
    // Let B settle (it errors without ever running).
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let frames = decoded_frames(&recorded);
    let capacity = frames
        .iter()
        .find(|f| f.kind == TaskFrameKind::Error && f.task_id == "t2")
        .expect("t2 should be rejected");
    assert!(
        capacity.text.contains("at capacity"),
        "got: {}",
        capacity.text
    );

    gate.notify_waiters();
    runtime.idle().await;
}

#[tokio::test]
async fn rejects_duplicate_task_id_from_same_sender() {
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let run_task = blocking_runner(ready_tx, gate.clone());
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("dup", "one", None)),
    );
    wait_ready(&mut ready_rx).await;

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("dup", "two", None)),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let frames = decoded_frames(&recorded);
    let dup_error = frames
        .iter()
        .find(|f| f.kind == TaskFrameKind::Error && f.task_id == "dup")
        .expect("duplicate should error");
    assert!(
        dup_error.text.contains("already running"),
        "got: {}",
        dup_error.text
    );
    // The original ack is still present.
    assert!(frames
        .iter()
        .any(|f| f.kind == TaskFrameKind::Ack && f.text == "task accepted"));

    gate.notify_waiters();
    runtime.idle().await;
}

#[tokio::test]
async fn forwards_input_into_running_task() {
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let received = Arc::new(StdMutex::new(Vec::new()));
    let run_task = stdin_runner(ready_tx, gate.clone(), received.clone());
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    wait_ready(&mut ready_rx).await;

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(input_frame("t1", "extra guidance", None)),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(
        received.lock().unwrap().as_slice(),
        &["extra guidance".to_string()]
    );
    let frames = decoded_frames(&recorded);
    assert!(frames
        .iter()
        .any(|f| f.kind == TaskFrameKind::Ack && f.text == "input received"));

    gate.notify_waiters();
    runtime.idle().await;
}

#[tokio::test]
async fn input_with_mismatched_correlation_does_not_match() {
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let received = Arc::new(StdMutex::new(Vec::new()));
    let run_task = stdin_runner(ready_tx, gate.clone(), received.clone());
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", Some("corr-A"))),
    );
    wait_ready(&mut ready_rx).await;

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(input_frame("t1", "wrong dispatch", Some("corr-B"))),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(
        received.lock().unwrap().is_empty(),
        "mismatched correlation must not forward"
    );
    let frames = decoded_frames(&recorded);
    assert!(frames
        .iter()
        .any(|f| f.kind == TaskFrameKind::Ack && f.text == "no matching running task for input"));

    gate.notify_waiters();
    runtime.idle().await;
}

#[tokio::test]
async fn shutdown_aborts_in_flight_task() {
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let run_task = abortable_runner(ready_tx);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    wait_ready(&mut ready_rx).await;
    assert_eq!(runtime.active_count(), 1, "one task in flight");

    runtime.shutdown();
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    assert!(frames
        .iter()
        .any(|f| f.kind == TaskFrameKind::Error && f.text.contains("aborted")));
    assert_eq!(runtime.active_count(), 0, "no tasks after shutdown");
}

#[tokio::test]
async fn input_for_unknown_task_is_not_matched() {
    let run_task: RunTaskFn = Arc::new(|opts: RunTaskOptions| {
        Box::pin(async move {
            Ok(RunTaskResult {
                usage: None,
                provider: opts.provider,
                reply: "ok".to_string(),
                events: 0,
            })
        })
    });
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(input_frame("ghost", "hi", None)),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    assert!(frames
        .iter()
        .any(|f| f.kind == TaskFrameKind::Ack && f.text == "no matching running task for input"));
}

#[tokio::test]
async fn input_buffered_before_stdin_registration_is_drained() {
    // A runner that starts (ready) but registers its stdin sink only after a gate
    // is released — so an `input` arriving in between must buffer in pending_input
    // and flush when the sink registers.
    let ready = Arc::new(Notify::new());
    let gate = Arc::new(Notify::new());
    let received = Arc::new(StdMutex::new(Vec::new()));
    let run_task: RunTaskFn = {
        let ready = ready.clone();
        let gate = gate.clone();
        let received = received.clone();
        Arc::new(move |mut opts: RunTaskOptions| {
            let ready = ready.clone();
            let gate = gate.clone();
            let received = received.clone();
            Box::pin(async move {
                ready.notify_waiters();
                gate.notified().await; // hold off registration until released
                let (tx, mut rx) = mpsc::unbounded_channel::<String>();
                if let Some(register) = opts.on_stdin.take() {
                    register(tx); // drains any buffered pending_input into tx
                }
                let sink = received.clone();
                let reader = tokio::spawn(async move {
                    while let Some(line) = rx.recv().await {
                        sink.lock().unwrap().push(line);
                    }
                });
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                reader.abort();
                Ok(RunTaskResult {
                    usage: None,
                    provider: opts.provider,
                    reply: "done".to_string(),
                    events: 0,
                })
            })
        })
    };
    let (send, _recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    ready.notified().await;
    // Input arrives before the stdin sink exists → buffered as pending_input.
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(input_frame("t1", "buffered guidance", None)),
    );
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    gate.notify_waiters(); // now registration drains the buffer
    runtime.idle().await;

    assert_eq!(
        received.lock().unwrap().as_slice(),
        &["buffered guidance".to_string()]
    );
}
