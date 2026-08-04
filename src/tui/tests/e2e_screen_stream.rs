//! End-to-end for `medulla.screen.v1`: a real harness on a real pty, sampled,
//! diffed, encoded, and folded into the hub's store.
//!
//! Every other test of this feature exercises one side against literal grids.
//! This is the only one that runs the whole chain — a child process writing to a
//! pseudo-terminal, a `vt100` emulator parsing it, the conversion into the wire
//! model, the diff, the JSON envelope, the hub's fold, and back out as the text
//! the child actually printed.
//!
//! Everything except the network: the transport is a channel rather than the
//! Signal relay, which is deliberate. What the relay does with an encrypted body
//! is proven separately and live in `medulla`'s `live_inbox_push`; what is
//! unproven without this is whether the two halves of the screen protocol agree
//! about a screen that came from a real terminal rather than one written by
//! hand.
//!
//! Unix-only: it runs `/bin/sh` on a pty, which Windows has no equivalent of.

#![cfg(unix)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use medulla::daemon::{DaemonConfig, DaemonRuntime};
use medulla::hub::ScreenStore;
use medulla::protocol::{
    parse_screen_message, ApplyOutcome, HarnessProvider, ScreenMessage, TaskFrameKind,
};
use medulla_tui::worker::pty::{HarnessControl, LaunchSpec, PtyManager};
use medulla_tui::worker::stream::{send_fn, ScreenRouter};

/// How long to allow for a child to paint and a frame to be sampled.
///
/// Generous on purpose: real children on real ptys are at the mercy of machine
/// load, and a tight deadline turns "the box was busy" into a red test.
const PATIENCE: Duration = Duration::from_secs(20);

/// A spec that runs `sh -c <script>` on a pty.
///
/// Codex rather than Claude: claude's interactive argv carries a minted
/// `--session-id`, which `/bin/sh` would reject as an unknown option. Codex
/// takes no preset id, so the script is the whole command.
fn sh(script: &str, label: &str) -> LaunchSpec {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    LaunchSpec {
        provider: HarnessProvider::Codex,
        bin: "/bin/sh".to_string(),
        cwd: "/".to_string(),
        env,
        extra_args: vec!["-c".to_string(), script.to_string()],
        skip_permissions: false,
        label: label.to_string(),
        session_id: None,
        model: None,
        control: HarnessControl::Orchestrator,
        user_spawned: false,
    }
}

/// A daemon runtime whose "executor" reports the pty it was handed and then
/// stays running, which is what a real task looks like while it is being
/// watched.
///
/// The runtime is the piece under test here as much as the sampler: it owns the
/// running-task record keyed by `(sender, task id)`, and that record is the only
/// thing that lets a subscription resolve.
fn runtime_serving(sessions: PtyManager, session_id: String) -> DaemonRuntime {
    let config = DaemonConfig {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        providers: vec![HarnessProvider::Codex],
        default_provider: HarnessProvider::Codex,
        workspace: "/tmp".into(),
        env: HashMap::new(),
        task_timeout_ms: 60_000,
        capability_timeout_ms: None,
        concurrency: 1,
        status_throttle_ms: 60_000,
        max_pending: 4,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        accessible_dirs: Vec::new(),
        router: None,
        custom_harnesses: Vec::new(),
        budget: None,
        attribution: true,
    };
    let run_task = Arc::new(move |options: medulla::daemon::providers::RunTaskOptions| {
        let session_id = session_id.clone();
        let sessions = sessions.clone();
        Box::pin(async move {
            if let Some(report) = options.on_session {
                report(session_id.clone());
            }
            // Hold the task open so its record — and therefore the subscription
            // resolving through it — stays live for the duration of the test.
            options.abort.cancelled().await;
            if options.abort.is_terminated() {
                sessions.close(&session_id);
            }
            Err("never settles".to_string())
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = _> + Send>>
    }) as medulla::daemon::providers::RunTaskFn;

    DaemonRuntime::new(config, run_task, Arc::new(|_, _| Box::pin(async {})))
}

/// Dispatch a task frame into `runtime` as `from`, and wait until it is running.
async fn start_task(runtime: &DaemonRuntime, from: &str, task_id: &str) {
    let body = medulla::protocol::encode_task_frame(medulla::protocol::EncodeFrameInput {
        kind: TaskFrameKind::Task,
        task_id: task_id.to_string(),
        text: "watch me".to_string(),
        ts: "2026-01-01T00:00:00Z".to_string(),
        correlation_id: Some(format!("cyc/{task_id}/0")),
        harness: None,
        provider: Some(HarnessProvider::Codex),
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    });
    let frame = medulla::protocol::decode_task_frame(&body);
    runtime.handle_message(from.to_string(), body, frame);

    let deadline = Instant::now() + PATIENCE;
    while runtime.session_for_task(from, task_id).is_none() {
        assert!(
            Instant::now() < deadline,
            "the runtime never recorded a session for the task"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Every screen message the worker sent, in order.
type Outbox = Arc<Mutex<Vec<(String, String)>>>;

#[tokio::test(flavor = "multi_thread")]
async fn a_watched_task_streams_its_real_terminal_to_the_hubs_store() {
    let peer = "peerA";
    let task_id = "t1#0";

    // A harness that paints something identifiable and then sits there, exactly
    // as a real one does between turns.
    let sessions = PtyManager::new();
    let session_id = sessions
        .open(sh(
            // Two paints, separated: the first crosses as a full frame, the
            // second can only arrive as a delta against it.
            "printf 'BOOTING-HARNESS\\n'; sleep 1; printf 'SECOND-PAINT\\n'; sleep 30",
            peer,
        ))
        .expect("a pty session");

    let runtime = runtime_serving(sessions.clone(), session_id.clone());
    start_task(&runtime, peer, task_id).await;

    let outbox: Outbox = Arc::new(Mutex::new(Vec::new()));
    let capture = outbox.clone();
    let mut router = ScreenRouter::new(
        sessions.clone(),
        runtime.clone(),
        send_fn(move |to, body| {
            let capture = capture.clone();
            async move {
                capture.lock().unwrap().push((to, body));
            }
        }),
    );

    // The hub subscribes to the task it dispatched.
    router.handle(
        peer,
        ScreenMessage::Subscribe {
            task_id: task_id.to_string(),
            max_fps: 10,
            resync: true,
        },
    );
    assert_eq!(router.active(), 1, "the subscription should have resolved");

    // Fold whatever arrives into a hub store, exactly as the pump does, until
    // the child's output shows up on the synchronised screen.
    let store = ScreenStore::new();
    let deadline = Instant::now() + PATIENCE;
    let mut applied = 0usize;
    let mut deltas = 0usize;
    let mut smallest_delta = usize::MAX;
    let mut seen_text = false;
    while Instant::now() < deadline && !seen_text {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let pending: Vec<(String, String)> = std::mem::take(&mut *outbox.lock().unwrap());
        for (to, body) in pending {
            assert_eq!(to, peer, "frames go to the subscriber");
            let Some(ScreenMessage::Frame(frame)) = parse_screen_message(&body) else {
                panic!("the worker should only send screen frames: {body}");
            };
            assert_eq!(frame.task_id, task_id, "frames are addressed by task");
            assert_eq!(
                store.apply(peer, &frame, 0),
                ApplyOutcome::Applied,
                "every frame the worker emits must apply in order"
            );
            if !frame.full {
                deltas += 1;
                smallest_delta = smallest_delta.min(frame.rows_changed.len());
            }
            applied += 1;
        }
        if let Some(held) = store.get(peer, task_id) {
            let text: String = held
                .grid
                .lines
                .iter()
                .flatten()
                .map(|run| run.text.as_str())
                .collect();
            // Both paints, so the delta carrying the second was applied on top
            // of the full frame carrying the first rather than replacing it.
            seen_text = text.contains("BOOTING-HARNESS") && text.contains("SECOND-PAINT");
        }
    }

    assert!(applied > 0, "no frames were streamed at all");
    assert!(
        deltas > 0,
        "only full frames crossed; the diff path was never exercised"
    );
    // The point of the whole protocol: a second paint costs a row or two, not a
    // whole 120x30 screen.
    assert!(
        smallest_delta <= 4,
        "a one-line paint resent {smallest_delta} rows"
    );
    assert!(
        seen_text,
        "the hub's screen never showed what the child printed; held: {:?}",
        store.get(peer, task_id).map(|h| h.grid.lines)
    );

    // The screen the hub holds is the worker's, at the worker's geometry.
    let held = store.get(peer, task_id).expect("a screen");
    assert_eq!(held.task_id, task_id);
    assert_eq!(
        held.grid.lines.len(),
        held.grid.rows as usize,
        "the held grid should be exactly as tall as it claims"
    );
    println!(
        "streamed {applied} frame(s) ({deltas} delta, smallest {smallest_delta} row(s)); \
         hub holds seq {} at {}x{}",
        held.seq, held.grid.cols, held.grid.rows
    );

    sessions.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unchanged_screen_costs_nothing_after_the_first_frame() {
    // The property the whole sampling design rests on: a watched-but-idle
    // session must stop putting frames on the wire. If this regresses, a fleet
    // of idle workers quietly saturates the transport.
    let peer = "peerA";
    let task_id = "t2#0";

    let sessions = PtyManager::new();
    let session_id = sessions
        .open(sh("printf 'STILL\\n'; sleep 30", peer))
        .expect("a pty session");
    let runtime = runtime_serving(sessions.clone(), session_id);
    start_task(&runtime, peer, task_id).await;

    let outbox: Outbox = Arc::new(Mutex::new(Vec::new()));
    let capture = outbox.clone();
    let mut router = ScreenRouter::new(
        sessions.clone(),
        runtime.clone(),
        send_fn(move |to, body| {
            let capture = capture.clone();
            async move {
                capture.lock().unwrap().push((to, body));
            }
        }),
    );
    router.handle(
        peer,
        ScreenMessage::Subscribe {
            task_id: task_id.to_string(),
            max_fps: 10,
            resync: true,
        },
    );

    // Let it settle: the child paints, the sampler sends, then nothing changes.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let settled = outbox.lock().unwrap().len();
    assert!(settled > 0, "the first frame should have gone out");

    // A full second at ten samples a second. A still screen must add nothing.
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    let after = outbox.lock().unwrap().len();
    assert_eq!(
        after,
        settled,
        "an unchanged screen sent {} further frame(s) across ~10 samples",
        after - settled
    );

    sessions.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owned_task_kill_stops_its_real_harness() {
    let peer = "peerA";
    let task_id = "t3#0";
    let sessions = PtyManager::new();
    let session_id = sessions.open(sh("sleep 30", peer)).expect("a pty session");
    let runtime = runtime_serving(sessions.clone(), session_id.clone());
    start_task(&runtime, peer, task_id).await;
    let mut router = ScreenRouter::new(sessions.clone(), runtime, send_fn(|_, _| async {}));

    router.handle(
        peer,
        ScreenMessage::Kill {
            task_id: task_id.to_string(),
            correlation_id: "stale-dispatch".to_string(),
        },
    );
    assert!(
        sessions
            .row(&session_id)
            .expect("the live session remains inspectable")
            .state
            .is_running(),
        "a stale dispatch receipt must not kill a reused task id"
    );

    router.handle(
        peer,
        ScreenMessage::Kill {
            task_id: task_id.to_string(),
            correlation_id: format!("cyc/{task_id}/0"),
        },
    );

    let deadline = Instant::now() + PATIENCE;
    while sessions
        .row(&session_id)
        .expect("the killed session remains inspectable")
        .state
        .is_running()
    {
        assert!(
            Instant::now() < deadline,
            "the task-scoped kill must stop the harness process"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(router.active(), 0, "its screen stream is also stopped");
    sessions.shutdown();
}
