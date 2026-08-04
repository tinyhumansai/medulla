//! End-to-end for the orchestrator's embedded harness pane: a task dispatched
//! into a host, served by a real child on a real pseudo-terminal, and resolved
//! back out by the pane the way the Agents tab resolves it.
//!
//! This is the chain the `-p` flow used to make impossible. Headless, a task
//! produced JSON and no screen, so an agent lane could only ever show a
//! transcript we reconstructed. Here the same dispatch produces a live terminal,
//! and what this test proves is that the pane can get from "the cursor is on
//! this task" to "these are the bytes that harness is painting" — through the
//! daemon's own `(sender, task id)` record, not by guessing.
//!
//! `/bin/sh` stands in for `claude`: it is a genuine pty client, so the
//! resolution, the emulator, the resize and the write side are all exercised
//! for real while the test stays fast, offline, and deterministic.
//!
//! Unix-only: it runs `/bin/sh` on a pty, which Windows has no equivalent of.

#![cfg(unix)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla::daemon::{DaemonConfig, DaemonRuntime};
use medulla::protocol::{HarnessProvider, TaskFrameKind};
use medulla_tui::ui::harness_pane::LocalHarnesses;
use medulla_tui::worker::pty::{HarnessControl, LaunchSpec, PtyManager};

/// How long to allow for a child to paint and a record to appear.
///
/// Generous on purpose: real children on real ptys are at the mercy of machine
/// load, and a tight deadline turns "the box was busy" into a red test.
const PATIENCE: Duration = Duration::from_secs(20);

/// The address local work is dispatched from, matching the hub's own.
const HUB: &str = "medulla-orchestrator";

/// A spec that runs `sh -c <script>` on a pty.
///
/// Codex rather than Claude: Claude's interactive argv carries a minted
/// `--session-id`, which `/bin/sh` would reject as an unknown option.
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

/// A runtime whose executor opens a real pty session for the task and reports
/// it, then holds the task open the way a harness mid-turn does.
///
/// The reporting is the part under test: `on_session` is how a session-backed
/// executor tells the daemon which terminal is serving which task, and that
/// record is the only thing that lets the pane resolve a screen at all.
fn runtime_over(sessions: PtyManager, script: &'static str) -> DaemonRuntime {
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
        let sessions = sessions.clone();
        Box::pin(async move {
            let id = sessions
                .open(sh(script, &options.conversation))
                .expect("a pty session");
            if let Some(report) = options.on_session {
                report(id);
            }
            // Hold the task open so its record — and therefore the pane's
            // resolution through it — stays live for the test.
            tokio::time::sleep(Duration::from_secs(60)).await;
            Err("never settles".to_string())
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = _> + Send>>
    }) as medulla::daemon::providers::RunTaskFn;

    DaemonRuntime::new(config, run_task, Arc::new(|_, _| Box::pin(async {})))
}

/// Dispatch a task frame into `runtime` as the hub would.
fn dispatch(runtime: &DaemonRuntime, task_id: &str) {
    let body = medulla::protocol::encode_task_frame(medulla::protocol::EncodeFrameInput {
        kind: TaskFrameKind::Task,
        task_id: task_id.to_string(),
        text: "do the thing".to_string(),
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
    runtime.handle_message(HUB.to_string(), body, frame);
}

/// Spin until `check` passes or the patience runs out.
async fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// The whole screen as one string.
fn text(harnesses: &LocalHarnesses, id: &str) -> String {
    harnesses
        .screen(id)
        .map(|snapshot| {
            snapshot
                .cells
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|c| c.text.as_str())
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dispatched_task_resolves_to_the_terminal_its_harness_is_painting() {
    let task_id = "t1#0";
    let sessions = PtyManager::new();
    let runtime = runtime_over(
        sessions.clone(),
        "printf 'HARNESS-IS-PAINTING\\n'; sleep 30",
    );
    let harnesses = LocalHarnesses {
        sessions: sessions.clone(),
        runtimes: std::sync::Arc::new(std::sync::Mutex::new(vec![runtime.clone()])),
        hub_address: HUB.to_string(),
        env: HashMap::new(),
        workspace: "/".to_string(),
        providers: vec![HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
        hooks: medulla::harness_hooks::HooksConfig::default(),
    };

    // Before dispatch there is nothing to show — the pane must not invent a
    // screen for work that has not started.
    assert_eq!(harnesses.session_for_task(task_id), None);

    dispatch(&runtime, task_id);

    wait_for("the task to name its session", || {
        harnesses.session_for_task(task_id).is_some()
    })
    .await;
    let id = harnesses.session_for_task(task_id).expect("a session");

    assert!(harnesses.is_running(&id));
    wait_for("the harness's own output to reach the pane", || {
        text(&harnesses, &id).contains("HARNESS-IS-PAINTING")
    })
    .await;

    // The pane fits the child to itself every frame; the child must actually
    // move, or the emulator and the terminal would disagree about the geometry
    // and the screen would render torn.
    harnesses.fit(&id, 96, 28);
    let snapshot = harnesses.screen(&id).expect("a screen");
    assert_eq!(snapshot.cells.len(), 28);
    assert_eq!(snapshot.cells[0].len(), 96);

    sessions.close(&id);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_attached_pane_types_into_the_harness_serving_the_task() {
    let task_id = "t2#0";
    let sessions = PtyManager::new();
    let runtime = runtime_over(
        sessions.clone(),
        "read line; printf 'typed:%s\\n' \"$line\"; sleep 30",
    );
    let harnesses = LocalHarnesses {
        sessions: sessions.clone(),
        runtimes: std::sync::Arc::new(std::sync::Mutex::new(vec![runtime.clone()])),
        hub_address: HUB.to_string(),
        env: HashMap::new(),
        workspace: "/".to_string(),
        providers: vec![HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
        hooks: medulla::harness_hooks::HooksConfig::default(),
    };

    dispatch(&runtime, task_id);
    wait_for("the task to name its session", || {
        harnesses.session_for_task(task_id).is_some()
    })
    .await;
    let id = harnesses.session_for_task(task_id).expect("a session");

    // Exactly what the attached pane writes: the encoder's bytes for typing
    // `hi` and pressing Enter. A newline instead of the carriage return would
    // leave the line sitting unsubmitted, which is the failure this pins.
    harnesses.write(&id, b"hi").expect("the pty accepts input");
    harnesses.write(&id, b"\r").expect("the pty accepts Enter");

    wait_for("the harness to act on what was typed", || {
        text(&harnesses, &id).contains("typed:hi")
    })
    .await;

    sessions.close(&id);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_task_that_names_no_session_shows_no_screen_rather_than_someone_elses() {
    let sessions = PtyManager::new();
    let runtime = runtime_over(sessions.clone(), "sleep 30");
    let harnesses = LocalHarnesses {
        sessions: sessions.clone(),
        runtimes: std::sync::Arc::new(std::sync::Mutex::new(vec![runtime.clone()])),
        hub_address: HUB.to_string(),
        env: HashMap::new(),
        workspace: "/".to_string(),
        providers: vec![HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
        hooks: medulla::harness_hooks::HooksConfig::default(),
    };

    dispatch(&runtime, "mine#0");
    wait_for("the dispatched task to name its session", || {
        harnesses.session_for_task("mine#0").is_some()
    })
    .await;

    // A task this host was never sent resolves to nothing, even though a
    // session is running. Showing the one that happens to exist would put
    // another task's terminal under this row.
    assert_eq!(harnesses.session_for_task("not-mine#0"), None);

    let id = harnesses.session_for_task("mine#0").expect("a session");
    sessions.close(&id);
}
