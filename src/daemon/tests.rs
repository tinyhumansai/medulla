//! Runtime state-machine tests driven by a fake executor (no network, no CLIs).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use serde_json::json;
use tokio::sync::{mpsc, Notify};

use crate::tinyplace_support::{
    decode_task_frame, HarnessEvent, HarnessProvider, TaskFrame, TaskFrameKind, TINYPLACE_PROTO,
};

use super::mappers::HarnessSemanticEvent;
use super::providers::{RunTaskOptions, RunTaskResult};
use super::*;

type Recorded = Arc<StdMutex<Vec<(String, String)>>>;

fn recording_send() -> (SendFn, Recorded) {
    let recorded: Recorded = Arc::new(StdMutex::new(Vec::new()));
    let sink = recorded.clone();
    let send: SendFn = Arc::new(move |to: String, body: String| {
        let sink = sink.clone();
        Box::pin(async move {
            sink.lock().unwrap().push((to, body));
        })
    });
    (send, recorded)
}

fn decoded_frames(recorded: &Recorded) -> Vec<TaskFrame> {
    recorded
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, body)| decode_task_frame(body))
        .collect()
}

fn base_config() -> DaemonConfig {
    DaemonConfig {
        providers: vec![HarnessProvider::Claude],
        default_provider: HarnessProvider::Claude,
        workspace: ".".to_string(),
        env: std::collections::HashMap::new(),
        task_timeout_ms: 60_000,
        capability_timeout_ms: None,
        concurrency: 2,
        status_throttle_ms: 4_000,
        max_pending: 16,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
    }
}

fn task_frame(task_id: &str, text: &str, correlation: Option<&str>) -> TaskFrame {
    TaskFrame {
        usage: None,
        proto: TINYPLACE_PROTO.to_string(),
        kind: TaskFrameKind::Task,
        task_id: task_id.to_string(),
        text: text.to_string(),
        ts: "2026-07-05T00:00:00Z".to_string(),
        correlation_id: correlation.map(str::to_string),
        harness: None,
        provider: None,
    }
}

fn input_frame(task_id: &str, text: &str, correlation: Option<&str>) -> TaskFrame {
    TaskFrame {
        usage: None,
        kind: TaskFrameKind::Input,
        ..task_frame(task_id, text, correlation)
    }
}

/// A runner that signals readiness, then blocks until `gate` is released.
fn blocking_runner(ready: mpsc::UnboundedSender<()>, gate: Arc<Notify>) -> RunTaskFn {
    Arc::new(move |opts: RunTaskOptions| {
        let ready = ready.clone();
        let gate = gate.clone();
        Box::pin(async move {
            let _ = ready.send(());
            gate.notified().await;
            Ok(RunTaskResult {
                usage: None,
                provider: opts.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    })
}

/// A runner that registers a stdin sink, records forwarded lines, and blocks.
fn stdin_runner(
    ready: mpsc::UnboundedSender<()>,
    gate: Arc<Notify>,
    received: Arc<StdMutex<Vec<String>>>,
) -> RunTaskFn {
    Arc::new(move |mut opts: RunTaskOptions| {
        let ready = ready.clone();
        let gate = gate.clone();
        let received = received.clone();
        Box::pin(async move {
            let (tx, mut rx) = mpsc::unbounded_channel::<String>();
            if let Some(register) = opts.on_stdin.take() {
                register(tx);
            }
            let sink = received.clone();
            let reader = tokio::spawn(async move {
                while let Some(line) = rx.recv().await {
                    sink.lock().unwrap().push(line);
                }
            });
            let _ = ready.send(());
            gate.notified().await;
            reader.abort();
            Ok(RunTaskResult {
                usage: None,
                provider: opts.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    })
}

fn tool_call_event() -> HarnessSemanticEvent {
    HarnessSemanticEvent {
        line: 0,
        timestamp_ms: 0,
        record_type: "assistant:tool_use".to_string(),
        event: HarnessEvent {
            kind: "tool_call".to_string(),
            role: "agent".to_string(),
            payload: json!({
                "call_id": "c1",
                "tool_name": "Bash",
                "tool_kind": "shell",
                "display": "ls -la",
                "input": {}
            }),
            ..Default::default()
        },
    }
}

/// A runner that fires `count` tool_call events at `on_event`, then replies.
fn status_runner(count: usize) -> RunTaskFn {
    Arc::new(move |mut opts: RunTaskOptions| {
        Box::pin(async move {
            if let Some(mut on_event) = opts.on_event.take() {
                let event = tool_call_event();
                for _ in 0..count {
                    on_event(&event);
                }
            }
            Ok(RunTaskResult {
                usage: None,
                provider: opts.provider,
                reply: "ok".to_string(),
                events: count,
            })
        })
    })
}

async fn wait_ready(rx: &mut mpsc::UnboundedReceiver<()>) {
    tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("runner did not signal readiness in time");
}

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
async fn throttles_status_frames() {
    let run_task = status_runner(3);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    // now() sequence: first event passes (10000 - MIN ≥ throttle), the next two
    // fall inside the 4s window relative to 10000, so only one status is emitted.
    let seq = Arc::new(vec![10_000i64, 11_000, 12_000]);
    let index = Arc::new(AtomicUsize::new(0));
    let now: NowFn = Arc::new(move || {
        let position = index.fetch_add(1, Ordering::SeqCst);
        *seq.get(position).unwrap_or(seq.last().unwrap())
    });
    let runtime = runtime.with_now(now);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    let status_count = frames
        .iter()
        .filter(|f| f.kind == TaskFrameKind::Status)
        .count();
    assert_eq!(
        status_count, 1,
        "exactly one status frame should survive throttling"
    );
    assert!(frames
        .iter()
        .any(|f| f.kind == TaskFrameKind::Reply && f.text == "ok"));
}

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

/// A runner that returns Err once its abort is signalled (models a real run
/// terminating on shutdown).
fn abortable_runner(ready: mpsc::UnboundedSender<()>) -> RunTaskFn {
    Arc::new(move |opts: RunTaskOptions| {
        let ready = ready.clone();
        Box::pin(async move {
            let _ = ready.send(());
            opts.abort.cancelled().await;
            Err::<RunTaskResult, String>("claude task aborted".to_string())
        })
    })
}

/// A runner that echoes a fixed capability JSON reply and counts invocations.
fn counting_capability_runner(count: Arc<AtomicUsize>) -> RunTaskFn {
    Arc::new(move |opts: RunTaskOptions| {
        let count = count.clone();
        Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(RunTaskResult {
                usage: None,
                provider: opts.provider,
                reply:
                    r#"{"tools":["Bash"],"mcpServers":[],"accessibleDirs":[],"summary":"probe"}"#
                        .to_string(),
                events: 0,
            })
        })
    })
}

fn capabilities_frame(task_id: &str, correlation: Option<&str>) -> TaskFrame {
    TaskFrame {
        usage: None,
        kind: TaskFrameKind::Capabilities,
        ..task_frame(task_id, "", correlation)
    }
}

#[tokio::test]
async fn capabilities_probe_is_cached_across_askers() {
    let count = Arc::new(AtomicUsize::new(0));
    let run_task = counting_capability_runner(count.clone());
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(capabilities_frame("c1", None)),
    );
    runtime.idle().await;
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(capabilities_frame("c2", None)),
    );
    runtime.idle().await;

    // Two result frames, but the underlying probe ran exactly once (cached).
    let frames = decoded_frames(&recorded);
    let results = frames
        .iter()
        .filter(|f| f.kind == TaskFrameKind::CapabilitiesResult)
        .count();
    assert_eq!(results, 2, "each asker gets a capabilities_result");
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "probe cached after first run"
    );
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

#[tokio::test]
async fn status_detail_maps_event_kinds() {
    let tool_call = tool_call_event().event;
    assert_eq!(
        status_detail(&tool_call).as_deref(),
        Some("running Bash: ls -la")
    );

    let thinking = HarnessEvent {
        kind: "agent_thinking".to_string(),
        role: "agent".to_string(),
        payload: json!({ "text": "hmm" }),
        ..Default::default()
    };
    assert_eq!(status_detail(&thinking).as_deref(), Some("thinking"));

    let message = HarnessEvent {
        kind: "agent_message".to_string(),
        payload: json!({ "text": "done" }),
        ..Default::default()
    };
    assert_eq!(status_detail(&message).as_deref(), Some("writing response"));

    let failed_tool = HarnessEvent {
        kind: "tool_result".to_string(),
        payload: json!({ "call_id": "c", "ok": false, "is_error": true, "output": "", "output_bytes": 0 }),
        ..Default::default()
    };
    assert_eq!(status_detail(&failed_tool).as_deref(), Some("tool failed"));
}
