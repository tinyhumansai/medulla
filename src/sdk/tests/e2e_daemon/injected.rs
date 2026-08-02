//! Daemon e2e over injected (deterministic, non-spawning) `run_task` runners:
//! opencode input rejection, capacity + duplicate rejection, the `idle()` drain
//! contract, and per-task model-hint resolution.

use crate::helpers::*;
use crate::support::wait_until;

// 3b. Input rejection: opencode children get a null stdin (no mid-run input
//     channel), so an `input` frame must come back as an Error — never a false
//     "input received" ack that silently discards the guidance.
#[tokio::test]
async fn input_frame_for_opencode_is_rejected() {
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(
        config(HarnessProvider::Opencode, ".".into(), HashMap::new()),
        blocking_runner(ready_tx, gate.clone()),
        send,
    );

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "oc-in-1", "start", Some("c1"))),
    );
    tokio::time::timeout(T, ready_rx.recv()).await.unwrap();

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(
            TaskFrameKind::Input,
            "oc-in-1",
            "guidance",
            Some("c1"),
        )),
    );
    wait_until("input rejected", T, || {
        decoded_frames(&recorded)
            .iter()
            .any(|f| f.kind == TaskFrameKind::Error && f.task_id == "oc-in-1")
    })
    .await;
    let rejection = decoded_frames(&recorded)
        .into_iter()
        .find(|f| f.kind == TaskFrameKind::Error && f.task_id == "oc-in-1")
        .unwrap();
    assert!(
        rejection.text.contains("does not accept mid-run input"),
        "got: {}",
        rejection.text
    );
    assert_eq!(rejection.harness, Some(HarnessProvider::Opencode));
    gate.notify_waiters();
    runtime.idle().await;
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
                    session_id: None,
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

// A per-task `model` hint on the frame overrides the daemon's configured default;
// a task frame without one falls back to the config model.
#[tokio::test]
async fn per_task_model_overrides_config_default() {
    let (send, _recorded) = recording_send();
    let (run_task, seen) = recording_model_run_task();
    let mut cfg = config(HarnessProvider::Claude, ".".into(), HashMap::new());
    cfg.model = Some("config/default".to_string());
    let runtime = DaemonRuntime::new(cfg, run_task, send);

    let mut override_frame = frame(TaskFrameKind::Task, "m-1", "do it", None);
    override_frame.model = Some("task/override".to_string());
    runtime.handle_message("peer".into(), String::new(), Some(override_frame));
    runtime.idle().await;

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "m-2", "do it too", None)),
    );
    runtime.idle().await;

    let models = seen.lock().unwrap().clone();
    assert_eq!(
        models,
        vec![
            Some("task/override".to_string()),
            Some("config/default".to_string()),
        ],
        "frame model wins over config; absent falls back to config"
    );
}

// 8. Liveness: a heartbeat must survive an executor that blocks its thread.
//
// Regression. The status consumer is spawned by the dispatcher and the executor
// is invoked on the very next line, so the consumer lands in that worker's LIFO
// slot — which tokio does not let other workers steal. An executor that blocked
// its thread rather than awaiting therefore stranded it there for the whole
// turn, and the heartbeat never fired even with idle workers to spare. A peer
// saw an ack and then permanent silence, which its no-progress watchdog cannot
// distinguish from a worker that died.
//
// Multi-threaded on purpose: the hazard is a stranded task on a *busy* worker
// while others idle, which a current-thread runtime cannot express.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_blocking_executor_still_heartbeats() {
    let (send, recorded) = recording_send();
    let mut cfg = config(HarnessProvider::Claude, ".".into(), HashMap::new());
    // heartbeat = throttle * 8, so 50ms gives a 400ms beat: several within the
    // window below, without the 32s a production throttle would impose.
    cfg.status_throttle_ms = 50;
    cfg.task_timeout_ms = 30_000;
    let runtime = DaemonRuntime::new(cfg, thread_blocking_runner(Duration::from_secs(3)), send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(frame(TaskFrameKind::Task, "hb-1", "start", Some("c1"))),
    );

    // The ack is not the point — it is sent before the executor is ever called,
    // so it arrives even when the consumer is stranded. The heartbeat behind it
    // is the liveness signal actually under test.
    wait_until("a heartbeat while the executor holds its thread", T, || {
        decoded_frames(&recorded)
            .iter()
            .any(|f| f.kind == TaskFrameKind::Status && f.text.contains("still working"))
    })
    .await;

    runtime.idle().await;
}
