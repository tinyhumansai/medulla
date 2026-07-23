//! Shared fixtures for the daemon e2e suites: a recording `send` sink, frame
//! decoding helpers, a `DaemonConfig` builder, and injectable `run_task`
//! runners (real-spawn, blocking, and model-recording). Re-exports the
//! std/tokio/medulla types the grouped test modules need so they can rely on a
//! single `use crate::helpers::*;`.

pub use std::collections::HashMap;
pub use std::sync::atomic::{AtomicUsize, Ordering};
pub use std::sync::{Arc, Mutex as StdMutex};
pub use std::time::Duration;

pub use tokio::sync::{mpsc, Notify};

pub use medulla::daemon::providers::{run_provider_task, RunTaskFn, RunTaskOptions, RunTaskResult};
pub use medulla::daemon::{DaemonConfig, DaemonRuntime, SendFn};
pub use medulla::tinyplace::{
    decode_task_frame, parse_agent_capabilities, HarnessProvider, TaskFrame, TaskFrameKind,
    TINYPLACE_PROTO,
};

/// Shared per-wait timeout for the daemon suites.
pub const T: Duration = Duration::from_secs(10);

/// A shared, thread-safe record of `(recipient, body)` pairs the daemon sent.
pub type Recorded = Arc<StdMutex<Vec<(String, String)>>>;

/// A `send` closure that records every outbound `(to, body)` into a shared vec.
pub fn recording_send() -> (SendFn, Recorded) {
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
pub fn decoded_frames(recorded: &Recorded) -> Vec<TaskFrame> {
    recorded
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, body)| decode_task_frame(body))
        .collect()
}

/// The raw recorded bodies, without frame decoding.
pub fn raw_bodies(recorded: &Recorded) -> Vec<String> {
    recorded
        .lock()
        .unwrap()
        .iter()
        .map(|(_, b)| b.clone())
        .collect()
}

/// A `DaemonConfig` for a single provider with a zero status throttle (so every
/// mapped event yields a status frame) and short timeouts.
pub fn config(
    provider: HarnessProvider,
    workspace: String,
    env: HashMap<String, String>,
) -> DaemonConfig {
    DaemonConfig {
        providers: vec![provider],
        default_provider: provider,
        workspace,
        env,
        task_timeout_ms: 5_000,
        capability_timeout_ms: Some(5_000),
        concurrency: 2,
        // Zero throttle so every mapped event yields a status frame.
        status_throttle_ms: 0,
        max_pending: 16,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
    }
}

/// A `run_task` that drives the REAL spawn path against a fake provider CLI.
pub fn real_run_task() -> RunTaskFn {
    Arc::new(|options: RunTaskOptions| Box::pin(run_provider_task(options)))
}

/// Build a task frame with a fixed timestamp and optional correlation id.
pub fn frame(
    kind: TaskFrameKind,
    task_id: &str,
    text: &str,
    correlation: Option<&str>,
) -> TaskFrame {
    TaskFrame {
        usage: None,
        proto: TINYPLACE_PROTO.to_string(),
        kind,
        task_id: task_id.to_string(),
        text: text.to_string(),
        ts: "2026-07-05T00:00:00Z".to_string(),
        correlation_id: correlation.map(str::to_string),
        harness: None,
        provider: None,
        model: None,
    }
}

/// A runner that signals readiness, then blocks until `gate` is released.
pub fn blocking_runner(ready: mpsc::UnboundedSender<()>, gate: Arc<Notify>) -> RunTaskFn {
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

/// A `run_task` that records each dispatch's requested model and returns at once
/// (no real spawn), so a test can assert which model the daemon resolved.
pub fn recording_model_run_task() -> (RunTaskFn, Arc<StdMutex<Vec<Option<String>>>>) {
    let seen: Arc<StdMutex<Vec<Option<String>>>> = Arc::new(StdMutex::new(Vec::new()));
    let sink = seen.clone();
    let run: RunTaskFn = Arc::new(move |opts: RunTaskOptions| {
        sink.lock().unwrap().push(opts.model.clone());
        Box::pin(async move {
            Ok(RunTaskResult {
                session_id: None,
                provider: opts.provider,
                reply: "ok".to_string(),
                events: 0,
                usage: None,
            })
        })
    });
    (run, seen)
}
