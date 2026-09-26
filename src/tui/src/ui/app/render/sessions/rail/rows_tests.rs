//! Tests for Sessions rail task rows: a dispatched task folded onto its live
//! harness, and what the row says when that harness fails.
//!
//! Unix-only: standing a real session up needs `/bin/sh` on a real pty, which
//! Windows has no equivalent of. The row model under test is portable; only the
//! way this file puts a session on the manager is not.

#![cfg(unix)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla::daemon::{DaemonConfig, DaemonRuntime};
use medulla::protocol::{HarnessProvider, TaskFrameKind};
use medulla::ui::agents::{TaskState, TaskStatus};

use super::super::super::super::rail::{RailRow, SessionRailRow};
use super::super::super::color;
use super::tests::{app, lane, NOW};
use crate::ui::harness_pane::LocalSessions;
use crate::worker::pty::{LaunchSpec, PtyManager, PtyState, SessionControl, SessionOrigin};

/// How long to allow a real child to exit and the daemon to record its session.
///
/// Generous on purpose: real children on real ptys are at the mercy of machine
/// load, and a tight deadline turns "the box was busy" into a red test.
const PATIENCE: Duration = Duration::from_secs(20);

/// A spec that runs `sh -c <script>` on a pty, as an orchestrator dispatch.
///
/// Codex rather than Claude: claude's interactive argv carries a minted
/// `--session-id`, which `/bin/sh` would reject as an unknown option. Codex
/// takes no preset id, so the script is the whole command.
fn sh(script: &str) -> LaunchSpec {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    LaunchSpec {
        provider: HarnessProvider::Codex,
        preset: None,
        bin: "/bin/sh".to_string(),
        cwd: "/".to_string(),
        env,
        extra_args: vec!["-c".to_string(), script.to_string()],
        skip_permissions: false,
        label: "t1#0".to_string(),
        session_id: None,
        model: None,
        control: SessionControl::Orchestrator,
        origin: SessionOrigin::Orchestrator,
        name: None,
        mcp_grant_session: None,
    }
}

/// A daemon runtime that reports `session_id` as the session serving any task it
/// is asked to run, then holds that task open — the shape of a real dispatch
/// while it is running.
fn runtime_serving(session_id: String) -> DaemonRuntime {
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
        Box::pin(async move {
            if let Some(report) = options.on_session {
                report(session_id.clone());
            }
            // Hold the task open so its record — and therefore the session
            // mapping — stays live for the duration of the test.
            options.abort.cancelled().await;
            Err("never settles".to_string())
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = _> + Send>>
    }) as medulla::daemon::providers::RunTaskFn;
    DaemonRuntime::new(config, run_task, Arc::new(|_, _| Box::pin(async {})))
}

/// Dispatch `task_id` to `runtime` as `from`, and wait until it records
/// `session_id` as the session serving it.
async fn start_task(runtime: &DaemonRuntime, from: &str, task_id: &str, session_id: &str) {
    let body = medulla::protocol::encode_task_frame(medulla::protocol::EncodeFrameInput {
        transport: None,
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
    while runtime.session_for_task(from, task_id).as_deref() != Some(session_id) {
        assert!(
            Instant::now() < deadline,
            "the runtime never recorded {session_id} for {task_id}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The one task-row decision this file pins down: a backing session that died is
/// bad news, so the row draws red with the failure explanation rather than the
/// generic yellow "needs input" used for a harness that merely wants input.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_backing_session_draws_the_task_row_red_with_the_explanation() {
    let manager = PtyManager::new();
    let id = manager.open(sh("exit 7")).expect("a session starts");

    // Wait for the child to exit and the reader thread to reap it.
    let deadline = Instant::now() + PATIENCE;
    loop {
        if manager.row(&id).is_some_and(|row| !row.state.is_running()) {
            break;
        }
        assert!(Instant::now() < deadline, "the child never exited");
        std::thread::sleep(Duration::from_millis(10));
    }
    let row = manager.row(&id).expect("the row is retained");
    assert!(
        matches!(row.state, PtyState::Exited { code: Some(code) } if code != 0),
        "exit 7 must reap as a nonzero exit: {:?}",
        row.state
    );
    assert!(
        !row.closed_by_request,
        "nothing asked this session to close"
    );

    let from = "medulla-orchestrator";
    let task_id = "t1#0";
    let runtime = runtime_serving(id.clone());
    start_task(&runtime, from, task_id, &id).await;

    // The app hosts the manager and the runtime that names its session for the
    // task, exactly as `hosting_app` does for a live local host.
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    let harnesses = LocalSessions {
        sessions: manager,
        runtimes: Arc::new(std::sync::Mutex::new(vec![runtime])),
        hub_address: from.to_string(),
        env,
        workspace: "/".to_string(),
        providers: vec![HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
        hooks: medulla::harness_hooks::HooksConfig::default(),
        log: None,
    };
    let mut app = app();
    app.set_local_sessions(harnesses);

    let waiting = std::collections::HashSet::from([id]);
    let task = TaskState {
        task_id: task_id.to_string(),
        status: TaskStatus::Running,
        turns: 0,
        last_at: 0,
        turn_blocks: Vec::new(),
        attention: None,
        question_id: None,
        work: None,
    };
    let row = RailRow::Session(Box::new(SessionRailRow {
        agent_id: None,
        lane_index: None,
        task: Some(task),
        local: None,
        last: true,
    }));

    let line = app.rail_row_line(&row, &[lane()], false, &waiting, NOW);
    let text = line.to_string();
    assert!(
        text.contains("exited with 7"),
        "the row must explain the failure, not hide it: {text}"
    );
    assert!(
        !text.contains("needs input"),
        "a dead harness is not waiting for input: {text}"
    );
    assert_eq!(
        line.spans[0].style.fg,
        Some(color("red")),
        "a failed backing session draws red, like every other failed surface"
    );
}
