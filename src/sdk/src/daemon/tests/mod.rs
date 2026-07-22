//! Runtime state-machine tests driven by a fake executor (no network, no CLIs),
//! split by surface:
//! [`task_tests`] covers task acceptance, dispatch, input forwarding, and
//! shutdown; [`provider_tests`] covers provider selection and plain-text DM
//! routing; [`capability_tests`] covers the cached capability probe, status
//! throttling, and semantic-event → status-line mapping.
//!
//! Shared test helpers (mock senders, config/frame constructors, and the runner
//! builders the child modules dispatch through) live here.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use serde_json::json;
use tokio::sync::{mpsc, Notify};

use crate::tinyplace::{
    decode_task_frame, HarnessEvent, HarnessProvider, TaskFrame, TaskFrameKind, TINYPLACE_PROTO,
};

use super::mappers::HarnessSemanticEvent;
use super::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use super::*;

mod capability_tests;
mod provider_tests;
mod task_tests;

/// Recorded `(recipient, body)` pairs captured by [`recording_send`].
pub(super) type Recorded = Arc<StdMutex<Vec<(String, String)>>>;

/// A [`SendFn`] that records every `(to, body)` it is handed, paired with the
/// shared sink the test reads back.
pub(super) fn recording_send() -> (SendFn, Recorded) {
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

/// Decode every recorded body that parses as a task frame.
pub(super) fn decoded_frames(recorded: &Recorded) -> Vec<TaskFrame> {
    recorded
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, body)| decode_task_frame(body))
        .collect()
}

/// A default daemon config offering only Claude, sized for the tests.
pub(super) fn base_config() -> DaemonConfig {
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

/// Build a `Task`-kind frame.
pub(super) fn task_frame(task_id: &str, text: &str, correlation: Option<&str>) -> TaskFrame {
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
        model: None,
    }
}

/// Build an `Input`-kind frame (otherwise identical to [`task_frame`]).
pub(super) fn input_frame(task_id: &str, text: &str, correlation: Option<&str>) -> TaskFrame {
    TaskFrame {
        usage: None,
        kind: TaskFrameKind::Input,
        ..task_frame(task_id, text, correlation)
    }
}

/// A runner that signals readiness, then blocks until `gate` is released.
pub(super) fn blocking_runner(ready: mpsc::UnboundedSender<()>, gate: Arc<Notify>) -> RunTaskFn {
    Arc::new(move |opts: RunTaskOptions| {
        let ready = ready.clone();
        let gate = gate.clone();
        Box::pin(async move {
            let _ = ready.send(());
            gate.notified().await;
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: opts.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    })
}

/// A runner that registers a stdin sink, records forwarded lines, and blocks.
pub(super) fn stdin_runner(
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
                session_id: None,
                usage: None,
                provider: opts.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    })
}

/// A representative `tool_call` semantic event used to exercise status derivation.
pub(super) fn tool_call_event() -> HarnessSemanticEvent {
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
pub(super) fn status_runner(count: usize) -> RunTaskFn {
    Arc::new(move |mut opts: RunTaskOptions| {
        Box::pin(async move {
            if let Some(mut on_event) = opts.on_event.take() {
                let event = tool_call_event();
                for _ in 0..count {
                    on_event(&event);
                }
            }
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: opts.provider,
                reply: "ok".to_string(),
                events: count,
            })
        })
    })
}

/// Await a runner's readiness signal, panicking if it never arrives.
pub(super) async fn wait_ready(rx: &mut mpsc::UnboundedReceiver<()>) {
    tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("runner did not signal readiness in time");
}

/// A runner that records the `conversation` each run was attributed to.
pub(super) fn conversation_runner(seen: Arc<StdMutex<Vec<String>>>) -> RunTaskFn {
    Arc::new(move |opts: RunTaskOptions| {
        seen.lock().unwrap().push(opts.conversation.clone());
        Box::pin(async move {
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: opts.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    })
}

/// A runner that returns Err once its abort is signalled (models a real run
/// terminating on shutdown).
pub(super) fn abortable_runner(ready: mpsc::UnboundedSender<()>) -> RunTaskFn {
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
pub(super) fn counting_capability_runner(count: Arc<AtomicUsize>) -> RunTaskFn {
    Arc::new(move |opts: RunTaskOptions| {
        let count = count.clone();
        Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(RunTaskResult {
                session_id: None,
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

/// Build a `Capabilities`-kind probe frame.
pub(super) fn capabilities_frame(task_id: &str, correlation: Option<&str>) -> TaskFrame {
    TaskFrame {
        usage: None,
        kind: TaskFrameKind::Capabilities,
        ..task_frame(task_id, "", correlation)
    }
}
