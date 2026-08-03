//! Provider-selection and plain-text DM routing tests: requesting an
//! unavailable provider, falling back when the default is absent, running the
//! default provider for a raw DM, and refusing plain text at capacity or with no
//! provider offered.

use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{mpsc, Notify};

use crate::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use crate::daemon::DaemonRuntime;
use crate::protocol::{decode_task_frame, HarnessProvider, TaskFrameKind};

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
                session_id: None,
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
                session_id: None,
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
async fn a_frame_that_overrides_the_provider_does_not_inherit_the_daemon_s_default_model() {
    // The daemon's own `model` pin is scoped to its *default* harness. A frame
    // that names a different provider and no model of its own (the shape a
    // paired harness/model resolution produces on purpose — see
    // `flow_engine::harness_choice`) must run that provider's own default
    // rather than being handed a model id chosen for a CLI it is not running.
    let seen_model: Arc<StdMutex<Option<Option<String>>>> = Arc::new(StdMutex::new(None));
    let capture = seen_model.clone();
    let run_task: RunTaskFn = Arc::new(move |opts: RunTaskOptions| {
        *capture.lock().unwrap() = Some(opts.model.clone());
        Box::pin(async move {
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: opts.provider,
                reply: "ok".to_string(),
                events: 0,
            })
        })
    });
    let (send, recorded) = recording_send();
    let mut config = base_config();
    config.providers = vec![HarnessProvider::Claude, HarnessProvider::Codex];
    config.default_provider = HarnessProvider::Claude;
    config.model = Some("claude-house-model".to_string());
    let runtime = DaemonRuntime::new(config, run_task, send);

    let mut frame = task_frame("t1", "work", None);
    frame.provider = Some(HarnessProvider::Codex);
    runtime.handle_message("peer".into(), String::new(), Some(frame));
    runtime.idle().await;

    assert_eq!(
        *seen_model.lock().unwrap(),
        Some(None),
        "the daemon's Claude-scoped model must not follow the task onto Codex"
    );
    let frames = decoded_frames(&recorded);
    assert!(
        frames
            .iter()
            .any(|f| f.kind == TaskFrameKind::Ack && f.harness == Some(HarnessProvider::Codex)),
        "the override must still take effect: {frames:?}"
    );
}

#[tokio::test]
async fn a_frame_that_explicitly_names_the_default_provider_still_gets_no_pinned_model() {
    // The subtler case: the frame's explicit choice happens to *equal* the
    // daemon's own default harness (`claude` on a Claude-default host). The
    // daemon cannot tell "no preference was stated" from "the same harness was
    // stated on purpose" by looking at the resolved provider alone — only
    // `frame.provider`/`custom_harness` (what was actually requested) says
    // that. An explicit `harness: "claude"` with no model must still get
    // Claude's own tool default, not this daemon's separately-pinned model.
    let seen_model: Arc<StdMutex<Option<Option<String>>>> = Arc::new(StdMutex::new(None));
    let capture = seen_model.clone();
    let run_task: RunTaskFn = Arc::new(move |opts: RunTaskOptions| {
        *capture.lock().unwrap() = Some(opts.model.clone());
        Box::pin(async move {
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: opts.provider,
                reply: "ok".to_string(),
                events: 0,
            })
        })
    });
    let (send, recorded) = recording_send();
    let mut config = base_config();
    config.model = Some("claude-house-opus".to_string());
    let runtime = DaemonRuntime::new(config, run_task, send);

    let mut frame = task_frame("t1", "work", None);
    frame.provider = Some(HarnessProvider::Claude);
    runtime.handle_message("peer".into(), String::new(), Some(frame));
    runtime.idle().await;

    assert_eq!(
        *seen_model.lock().unwrap(),
        Some(None),
        "an explicit harness choice must get its own default, even when it \
         happens to match the daemon's default provider"
    );
    let _ = recorded;
}

#[tokio::test]
async fn a_frame_that_does_not_override_the_provider_still_gets_the_daemon_s_default_model() {
    // Unchanged behaviour: with no override, the daemon's own default harness
    // runs, so its pinned model still applies.
    let seen_model: Arc<StdMutex<Option<Option<String>>>> = Arc::new(StdMutex::new(None));
    let capture = seen_model.clone();
    let run_task: RunTaskFn = Arc::new(move |opts: RunTaskOptions| {
        *capture.lock().unwrap() = Some(opts.model.clone());
        Box::pin(async move {
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: opts.provider,
                reply: "ok".to_string(),
                events: 0,
            })
        })
    });
    let (send, recorded) = recording_send();
    let mut config = base_config();
    config.model = Some("claude-house-model".to_string());
    let runtime = DaemonRuntime::new(config, run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    runtime.idle().await;

    assert_eq!(
        *seen_model.lock().unwrap(),
        Some(Some("claude-house-model".to_string()))
    );
    let _ = recorded;
}

#[tokio::test]
async fn select_provider_falls_back_to_first_when_default_absent() {
    let run_task: RunTaskFn = Arc::new(|opts: RunTaskOptions| {
        Box::pin(async move {
            Ok(RunTaskResult {
                session_id: None,
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
