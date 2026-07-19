//! Provider-selection and plain-text DM routing tests: requesting an
//! unavailable provider, falling back when the default is absent, running the
//! default provider for a raw DM, and refusing plain text at capacity or with no
//! provider offered.

use std::sync::Arc;

use tokio::sync::{mpsc, Notify};

use crate::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use crate::daemon::DaemonRuntime;
use crate::tinyplace::{decode_task_frame, HarnessProvider, TaskFrameKind};

use super::{base_config, blocking_runner, decoded_frames, recording_send, task_frame, wait_ready};

#[tokio::test]
async fn no_provider_for_requested_errors_without_harness() {
    let (ready_tx, _ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let run_task = blocking_runner(ready_tx, gate);
    let (send, recorded) = recording_send();
    // Only claude is offered; request codex.
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    let mut frame = task_frame("t1", "work", None);
    frame.provider = Some(HarnessProvider::Codex);
    runtime.handle_message("peer".into(), String::new(), Some(frame));
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    let error = frames
        .iter()
        .find(|f| f.kind == TaskFrameKind::Error)
        .expect("should error");
    assert!(error.text.contains("no available provider"));
    assert!(error.text.contains("codex"));
    assert!(
        error.harness.is_none(),
        "provider-selection error carries no harness"
    );
}

#[tokio::test]
async fn plaintext_dm_runs_default_provider() {
    let run_task: RunTaskFn = Arc::new(|opts: RunTaskOptions| {
        Box::pin(async move {
            Ok(RunTaskResult {
                usage: None,
                provider: opts.provider,
                reply: format!("echo: {}", opts.prompt),
                events: 0,
            })
        })
    });
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message("peer".into(), "hello there".into(), None);
    runtime.idle().await;

    // Plain-text replies are sent raw (not task frames).
    let raw = recorded.lock().unwrap();
    assert!(raw.iter().any(|(_, body)| body == "echo: hello there"));
    assert!(decode_task_frame(&raw[0].1).is_none());
}

#[tokio::test]
async fn plaintext_without_available_provider_is_refused() {
    let run_task: RunTaskFn = Arc::new(|opts: RunTaskOptions| {
        Box::pin(async move {
            Ok(RunTaskResult {
                usage: None,
                provider: opts.provider,
                reply: "unreachable".to_string(),
                events: 0,
            })
        })
    });
    let (send, recorded) = recording_send();
    let mut config = base_config();
    config.providers = Vec::new(); // nothing offered
    let runtime = DaemonRuntime::new(config, run_task, send);

    runtime.handle_message("peer".into(), "hello".into(), None);
    runtime.idle().await;

    let bodies: Vec<String> = recorded
        .lock()
        .unwrap()
        .iter()
        .map(|(_, b)| b.clone())
        .collect();
    assert!(bodies
        .iter()
        .any(|b| b.contains("No coding agent is available")));
}

#[tokio::test]
async fn plaintext_at_capacity_is_refused() {
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let run_task = blocking_runner(ready_tx, gate.clone());
    let (send, recorded) = recording_send();
    let mut config = base_config();
    config.concurrency = 1;
    config.max_pending = 1;
    let runtime = DaemonRuntime::new(config, run_task, send);

    runtime.handle_message("peer".into(), "first".into(), None);
    wait_ready(&mut ready_rx).await;
    runtime.handle_message("peer".into(), "second".into(), None);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let bodies: Vec<String> = recorded
        .lock()
        .unwrap()
        .iter()
        .map(|(_, b)| b.clone())
        .collect();
    assert!(bodies.iter().any(|b| b.contains("Daemon at capacity")));

    gate.notify_waiters();
    runtime.idle().await;
}

#[tokio::test]
async fn select_provider_falls_back_to_first_when_default_absent() {
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
    let mut config = base_config();
    // Only codex is offered but claude is (wrongly) the default → first wins.
    config.providers = vec![HarnessProvider::Codex];
    config.default_provider = HarnessProvider::Claude;
    let runtime = DaemonRuntime::new(config, run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    let ack = frames
        .iter()
        .find(|f| f.kind == TaskFrameKind::Ack)
        .expect("ack");
    assert_eq!(ack.harness, Some(HarnessProvider::Codex));
}
