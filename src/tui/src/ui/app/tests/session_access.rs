//! Regression coverage for attaching to and closing locally hosted sessions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use medulla::protocol::HarnessProvider;

use super::{app, tab};
use crate::ui::harness_pane::LocalSessions;
use crate::worker::pty::{LaunchSpec, PtyManager, SessionControl, SessionOrigin};

/// Install an empty hosting surface and return its shared session manager.
fn host(app: &mut super::App) -> PtyManager {
    let sessions = PtyManager::new();
    app.set_local_sessions(LocalSessions {
        sessions: sessions.clone(),
        runtimes: Arc::new(Mutex::new(Vec::new())),
        hub_address: "this-device".to_string(),
        env: HashMap::new(),
        workspace: "/".to_string(),
        providers: Vec::new(),
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
        hooks: app.loaded.config.hooks.clone(),
        log: None,
    });
    sessions
}

/// Open a deterministic shell session with the requested control owner.
fn open_session(sessions: &PtyManager, control: SessionControl) -> String {
    open_session_with_grant(sessions, control, None)
}

/// Open a deterministic shell session that also carries `grant` as its MCP
/// grant session, the same key a workflow run reports progress under.
fn open_session_with_grant(
    sessions: &PtyManager,
    control: SessionControl,
    grant: Option<&str>,
) -> String {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    sessions
        .open(LaunchSpec {
            provider: HarnessProvider::Codex,
            preset: None,
            bin: "/bin/sh".to_string(),
            cwd: "/".to_string(),
            env,
            extra_args: vec!["-c".to_string(), "sleep 30".to_string()],
            skip_permissions: false,
            label: "test".to_string(),
            session_id: None,
            model: None,
            control,
            origin: SessionOrigin::User,
            name: None,
            mcp_grant_session: grant.map(str::to_string),
        })
        .expect("open test session")
}

#[test]
fn enter_on_an_exited_hosted_harness_is_consumed_without_attaching() {
    let mut app = app();
    app.tab_index = tab("Sessions");
    let sessions = host(&mut app);
    let session = open_session(&sessions, SessionControl::User);
    assert!(sessions.close(&session));
    app.pane_session = Some(session);

    let cmd = app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(cmd.is_none());
    assert!(app.status().contains("exited"), "{}", app.status());
    assert_eq!(app.attached_session(), None);
}

#[test]
fn orchestrator_session_refuses_attachment_and_remains_running() {
    let mut app = app();
    app.tab_index = tab("Sessions");
    let sessions = host(&mut app);
    let session = open_session(&sessions, SessionControl::Orchestrator);
    app.pane_session = Some(session.clone());

    let cmd = app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(cmd.is_none());
    assert!(app.status().contains("view-only"), "{}", app.status());
    assert_eq!(app.attached_session(), None);
    assert!(sessions
        .row(&session)
        .is_some_and(|row| row.state.is_running()));
    sessions.close(&session);
}

#[test]
fn stale_close_prompt_cannot_kill_an_orchestrator_session() {
    let mut app = app();
    let sessions = host(&mut app);
    let session = open_session(&sessions, SessionControl::User);
    app.arm_harness_close(session.clone());
    assert!(sessions.set_control(&session, SessionControl::Orchestrator));

    app.close_session(&session);

    assert!(app.status().contains("view-only"), "{}", app.status());
    assert!(sessions
        .row(&session)
        .is_some_and(|row| row.state.is_running()));
    sessions.close(&session);
}

#[test]
fn k_on_an_exited_hosted_harness_drops_its_row() {
    // The gesture this pins: an exited session used to answer `k` with "that
    // session has already exited" and stay on the rail. For a *failed* child
    // that is a permanent row — `keeps_finished_session` pins the failure cue,
    // so the frame sweep will not take it either — leaving the operator a dead
    // entry and nothing that removes it.
    let mut app = app();
    app.tab_index = tab("Sessions");
    let sessions = host(&mut app);
    let session = open_session(&sessions, SessionControl::User);
    assert!(sessions.close(&session));
    app.pane_session = Some(session.clone());

    let cmd = app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));

    assert!(cmd.is_none());
    assert!(
        app.harness_close_armed.is_none(),
        "a settled child has no work to lose, so there is nothing to confirm"
    );
    assert!(sessions.row(&session).is_none(), "the record is released");
    assert!(app.status().contains("Dismissed"), "{}", app.status());
}

#[test]
fn dismissing_the_attached_session_hands_the_keyboard_back() {
    // Forgetting the row the keyboard is in would leave the attachment pointing
    // at a session the manager no longer has, and every keystroke after it
    // typing into nothing.
    let mut app = app();
    app.tab_index = tab("Sessions");
    let sessions = host(&mut app);
    let session = open_session(&sessions, SessionControl::User);
    assert!(sessions.close(&session));
    app.harness_focus = crate::ui::harness_pane::HarnessFocus::Attached(session.clone());
    app.pane_session = Some(session.clone());

    app.close_pane_session_prompt();

    assert_eq!(app.attached_session(), None);
    assert!(sessions.row(&session).is_none());
}

#[test]
fn dismissing_an_exited_session_with_an_active_run_is_refused() {
    // `keeps_finished_session` pins a settled child whose MCP grant still has
    // a run executing under it — a detached run outlives its parent harness by
    // design, and its rows are drawn under this session. `PtyManager::forget`
    // revokes that grant unconditionally, so dismissing the row here would pull
    // the run's reporting channel out from under it. The row must stay, and the
    // grant must not be forgotten, until the run settles.
    let mut app = app();
    app.tab_index = tab("Sessions");
    let sessions = host(&mut app);
    let session = open_session_with_grant(&sessions, SessionControl::User, Some("grant-1"));
    assert!(sessions.close(&session));
    app.pane_session = Some(session.clone());
    app.harness_runs.report(
        "grant-1",
        medulla::control_socket::RunReport {
            run_id: "run-1".into(),
            workflow_id: "workflow".into(),
            status: medulla::control_socket::HarnessRunStatus::Running,
            detail: None,
            node: None,
        },
    );

    app.close_pane_session_prompt();

    assert!(
        sessions.row(&session).is_some(),
        "the row must stay while its run is still executing"
    );
    assert!(app.status().contains("still executing"), "{}", app.status());
}

#[test]
fn a_settled_task_session_is_dismissable_despite_orchestrator_control() {
    // The view-only refusal speaks of task sessions that are *running*. Applied
    // to one that has exited it pinned the row exactly as the old
    // already-exited refusal did, from the branch above it.
    let mut app = app();
    app.tab_index = tab("Sessions");
    let sessions = host(&mut app);
    let session = open_session(&sessions, SessionControl::Orchestrator);
    assert!(sessions.close(&session));
    app.pane_session = Some(session.clone());

    app.close_pane_session_prompt();

    assert!(sessions.row(&session).is_none(), "{}", app.status());
}
