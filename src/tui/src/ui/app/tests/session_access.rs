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
            mcp_grant_session: None,
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
