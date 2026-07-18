//! End-to-end tests for the daemon [`DaemonRuntime`] with fake providers and an
//! in-memory transport (an injected recording `send` closure).
//!
//! Two execution modes are exercised:
//! - injected `run_task` (deterministic, for capacity/duplicate/drain), and
//! - the REAL spawn path ([`run_provider_task`]) driving fake provider CLIs
//!   (shell scripts emitting realistic JSONL) via `TINYPLACE_*_BIN` overrides.
//!
//! Nothing touches the network or tiny.place, and no real claude/codex/opencode
//! binary is required.

mod support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{mpsc, Notify};

use medulla::daemon::providers::{run_provider_task, RunTaskFn, RunTaskOptions, RunTaskResult};
use medulla::daemon::{DaemonConfig, DaemonRuntime, SendFn};
use medulla::tinyplace_support::{
    decode_task_frame, parse_agent_capabilities, HarnessProvider, TaskFrame, TaskFrameKind,
    TINYPLACE_PROTO,
};

use support::fake_provider::{
    claude_capabilities_script, claude_script, claude_stdin_echo_script, hanging_script,
    opencode_script, provider_env, TempDir,
};
use support::wait_until;

const T: Duration = Duration::from_secs(10);

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

fn raw_bodies(recorded: &Recorded) -> Vec<String> {
    recorded
        .lock()
        .unwrap()
        .iter()
        .map(|(_, b)| b.clone())
        .collect()
}

fn config(
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

fn real_run_task() -> RunTaskFn {
    Arc::new(|options: RunTaskOptions| Box::pin(run_provider_task(options)))
}

fn frame(kind: TaskFrameKind, task_id: &str, text: &str, correlation: Option<&str>) -> TaskFrame {
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
    }
}

// 1. Task frame lifecycle e2e via the real spawn path (claude).
//    ack("task accepted") → status* → reply, correlationId echoed, harness set,
//    reply text from the claude result line (precedence over the agent message).
#[tokio::test]
async fn task_lifecycle_ack_status_reply() {
    let tmp = TempDir::new();
    let script = tmp.write_script("claude", &claude_script("intermediate", "final answer"));
    let env = provider_env(&[("TINYPLACE_CLAUDE_BIN", &script)]);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Claude, tmp.path_str(), env),
        real_run_task(),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "cyc-1", "do it", Some("corr-1"))),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    assert!(!frames.is_empty());
    // First frame is the ack, last is the reply; the middle are all statuses.
    assert_eq!(frames[0].kind, TaskFrameKind::Ack);
    assert_eq!(frames[0].text, "task accepted");
    let last = frames.last().unwrap();
    assert_eq!(last.kind, TaskFrameKind::Reply);
    assert_eq!(
        last.text, "final answer",
        "claude result line wins over agent message"
    );
    let usage = last.usage.expect("reply carries the child's token usage");
    assert_eq!(usage.input_tokens, 1234);
    assert_eq!(usage.output_tokens, 56);
    for middle in &frames[1..frames.len() - 1] {
        assert_eq!(
            middle.kind,
            TaskFrameKind::Status,
            "middle frames are status"
        );
    }
    assert!(
        frames.iter().any(|f| f.kind == TaskFrameKind::Status),
        "at least one status frame"
    );
    // correlationId echoed + harness set on every frame.
    for f in &frames {
        assert_eq!(f.correlation_id.as_deref(), Some("corr-1"));
        assert_eq!(f.harness, Some(HarnessProvider::Claude));
    }
}

// 2. Fake provider via real spawn (opencode): tool_call/tool_result mapping into
//    status details, and the final text part as the reply.
#[tokio::test]
async fn opencode_tool_status_details_and_reply() {
    let tmp = TempDir::new();
    let script = tmp.write_script("opencode", &opencode_script("opencode done"));
    let env = provider_env(&[("TINYPLACE_OPENCODE_BIN", &script)]);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Opencode, tmp.path_str(), env),
        real_run_task(),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "oc-1", "read a file", None)),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    let statuses: Vec<String> = frames
        .iter()
        .filter(|f| f.kind == TaskFrameKind::Status)
        .map(|f| f.text.clone())
        .collect();
    assert!(
        statuses.iter().any(|s| s == "running read: /a/b.rs"),
        "tool_call → status: {statuses:?}"
    );
    assert!(
        statuses.iter().any(|s| s == "tool completed"),
        "tool_result → status: {statuses:?}"
    );
    let reply = frames.last().unwrap();
    assert_eq!(reply.kind, TaskFrameKind::Reply);
    assert_eq!(reply.text, "opencode done");
    assert_eq!(reply.harness, Some(HarnessProvider::Opencode));
}

// 3. Input forwarding: a stdin-reading fake provider gets a mid-task `input`
//    frame; the daemon acks it and the text reaches the child (visible in reply).
#[tokio::test]
async fn input_frame_reaches_provider_stdin() {
    let tmp = TempDir::new();
    let script = tmp.write_script("claude", &claude_stdin_echo_script());
    let env = provider_env(&[("TINYPLACE_CLAUDE_BIN", &script)]);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Claude, tmp.path_str(), env),
        real_run_task(),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "in-1", "start", Some("c9"))),
    );
    // Wait for the task ack (the running record exists by then, so input buffers).
    wait_until("task accepted", T, || {
        decoded_frames(&recorded)
            .iter()
            .any(|f| f.kind == TaskFrameKind::Ack && f.text == "task accepted")
    })
    .await;

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(
            TaskFrameKind::Input,
            "in-1",
            "extra guidance",
            Some("c9"),
        )),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    assert!(
        frames
            .iter()
            .any(|f| f.kind == TaskFrameKind::Ack && f.text == "input received"),
        "input should be acked"
    );
    let reply = frames
        .iter()
        .find(|f| f.kind == TaskFrameKind::Reply)
        .expect("reply");
    assert_eq!(
        reply.text, "got: extra guidance",
        "forwarded input reached the provider"
    );
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

// 4. Capacity + duplicate rejection.
#[tokio::test]
async fn capacity_and_duplicate_rejection() {
    // (a) Capacity: maxPending 1, a blocked task, a second task is rejected.
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let (send, recorded) = recording_send();
    let mut cfg = config(HarnessProvider::Claude, ".".into(), HashMap::new());
    cfg.concurrency = 1;
    cfg.max_pending = 1;
    let runtime = DaemonRuntime::new(cfg, blocking_runner(ready_tx, gate.clone()), send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "t1", "a", None)),
    );
    tokio::time::timeout(T, ready_rx.recv()).await.unwrap();

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "t2", "b", None)),
    );
    let capacity = {
        let recorded = recorded.clone();
        wait_until("capacity error", T, || {
            decoded_frames(&recorded)
                .iter()
                .any(|f| f.kind == TaskFrameKind::Error && f.task_id == "t2")
        })
        .await;
        decoded_frames(&recorded)
            .into_iter()
            .find(|f| f.kind == TaskFrameKind::Error && f.task_id == "t2")
            .unwrap()
    };
    assert!(
        capacity.text.contains("at capacity"),
        "got: {}",
        capacity.text
    );
    gate.notify_waiters();
    runtime.idle().await;

    // (b) Duplicate: maxPending high, same taskId from same sender → already running.
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Claude, ".".into(), HashMap::new()),
        blocking_runner(ready_tx, gate.clone()),
        send,
    );
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "dup", "one", None)),
    );
    tokio::time::timeout(T, ready_rx.recv()).await.unwrap();
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "dup", "two", None)),
    );
    wait_until("dup error", T, || {
        decoded_frames(&recorded)
            .iter()
            .any(|f| f.kind == TaskFrameKind::Error && f.task_id == "dup")
    })
    .await;
    let dup = decoded_frames(&recorded)
        .into_iter()
        .find(|f| f.kind == TaskFrameKind::Error && f.task_id == "dup")
        .unwrap();
    assert!(dup.text.contains("already running"), "got: {}", dup.text);
    gate.notify_waiters();
    runtime.idle().await;
}

// 5. Capabilities: a `capabilities` frame → capabilities_result with parseable
//    AgentCapabilities; daemon cwd wins over the report, tools come from report.
#[tokio::test]
async fn capabilities_result_merges_daemon_and_report() {
    let tmp = TempDir::new();
    let script = tmp.write_script("claude", &claude_capabilities_script());
    let env = provider_env(&[("TINYPLACE_CLAUDE_BIN", &script)]);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Claude, tmp.path_str(), env),
        real_run_task(),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(
            TaskFrameKind::Capabilities,
            "cap-1",
            "",
            Some("corr-9"),
        )),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    let result = frames
        .iter()
        .find(|f| f.kind == TaskFrameKind::CapabilitiesResult)
        .expect("capabilities_result frame");
    assert_eq!(result.correlation_id.as_deref(), Some("corr-9"));
    assert_eq!(result.harness, Some(HarnessProvider::Claude));

    let caps = parse_agent_capabilities(&result.text).expect("parseable capabilities");
    // cwd from the daemon (canonicalized workspace) wins.
    let expected_cwd = std::fs::canonicalize(tmp.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(caps.cwd.as_deref(), Some(expected_cwd.as_str()));
    // tools/mcpServers come from the report.
    assert_eq!(caps.tools, vec!["Bash", "Read"]);
    assert_eq!(caps.mcp_servers, vec!["langfuse"]);
    // accessibleDirs unions the daemon cwd with the reported dir.
    assert!(caps.accessible_dirs.contains(&expected_cwd));
    assert!(caps.accessible_dirs.iter().any(|d| d == "/opt/extra"));
    assert_eq!(caps.summary.as_deref(), Some("fake claude"));
}

// 6. Plaintext DM: a non-frame body → raw text reply (no frame) via real spawn.
#[tokio::test]
async fn plaintext_dm_replies_raw() {
    let tmp = TempDir::new();
    let script = tmp.write_script("claude", &claude_script("ignored", "plain reply"));
    let env = provider_env(&[("TINYPLACE_CLAUDE_BIN", &script)]);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Claude, tmp.path_str(), env),
        real_run_task(),
        send,
    );

    runtime.handle_message("peer".into(), "just chatting".into(), None);
    runtime.idle().await;

    let bodies = raw_bodies(&recorded);
    assert!(
        bodies.iter().any(|b| b == "plain reply"),
        "raw reply: {bodies:?}"
    );
    // The reply is not a protocol frame.
    for body in &bodies {
        assert!(
            decode_task_frame(body).is_none(),
            "plaintext reply must not be a frame"
        );
    }
}

// 7. Idle watchdog: a hanging fake provider with a tiny task timeout → error frame
//    mentioning idle; the child is killed so the test finishes fast.
#[tokio::test]
async fn idle_watchdog_kills_hung_provider() {
    let tmp = TempDir::new();
    let script = tmp.write_script("claude", &hanging_script());
    let env = provider_env(&[("TINYPLACE_CLAUDE_BIN", &script)]);
    let (send, recorded) = recording_send();
    let mut cfg = config(HarnessProvider::Claude, tmp.path_str(), env);
    cfg.task_timeout_ms = 300; // fire fast
    let runtime = DaemonRuntime::new(cfg, real_run_task(), send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "hang-1", "wait forever", None)),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    let error = frames
        .iter()
        .find(|f| f.kind == TaskFrameKind::Error)
        .expect("idle error frame");
    assert!(error.text.contains("idle"), "got: {}", error.text);
}

// 8. Drain semantics: `--once` itself is wired only through the CLI `run_daemon`
//    (network transport), so we exercise the library-level drain contract it
//    relies on — `idle()` resolves only once every dispatched message settled.
#[tokio::test]
async fn idle_drains_all_dispatched_messages() {
    let seen = Arc::new(AtomicUsize::new(0));
    let counting: RunTaskFn = {
        let seen = seen.clone();
        Arc::new(move |opts: RunTaskOptions| {
            let seen = seen.clone();
            Box::pin(async move {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(RunTaskResult {
                    usage: None,
                    provider: opts.provider,
                    reply: format!("echo:{}", opts.prompt),
                    events: 0,
                })
            })
        })
    };
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Claude, ".".into(), HashMap::new()),
        counting,
        send,
    );

    for i in 0..3 {
        runtime.handle_message("peer".into(), format!("msg{i}"), None);
    }
    runtime.idle().await;

    assert_eq!(
        seen.load(Ordering::SeqCst),
        3,
        "every dispatch ran before idle resolved"
    );
    let bodies = raw_bodies(&recorded);
    for i in 0..3 {
        assert!(
            bodies.iter().any(|b| b == &format!("echo:msg{i}")),
            "reply for msg{i}"
        );
    }
}
