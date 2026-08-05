//! Tab and list-cursor navigation, plus the state accessors the render and
//! event-loop layers read.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::super::pty::PtyManager;
use super::super::types::{ExecutionMode, WorkerCmd, TAB_MASTER};
use super::helpers::{app_with, headless_app, key, sh};
use medulla::protocol::HarnessProvider;

// ------------------------------------------------------------- navigation ---

#[test]
fn tab_and_number_keys_move_between_tabs() {
    let mut app = app_with(PtyManager::new());
    assert_eq!(app.tab(), "Agents");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.tab(), "Master");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.tab(), "Workspaces");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.tab(), "Agents", "wraps");
    app.on_key(key(KeyCode::Char('3')));
    assert_eq!(app.tab(), "Workspaces");
}

#[test]
fn backtab_walks_the_tabs_the_other_way() {
    // Shift-Tab is the only way back a step; without it the operator has to wrap
    // all the way round.
    let mut app = app_with(PtyManager::new());
    assert_eq!(app.tab(), "Agents");
    app.on_key(key(KeyCode::BackTab));
    assert_eq!(
        app.tab(),
        "Workspaces",
        "back-tab from the first tab wraps to the last"
    );
    app.on_key(key(KeyCode::BackTab));
    assert_eq!(app.tab(), "Master");
}

#[test]
fn q_and_ctrl_c_quit_the_running_worker() {
    let mut app = app_with(PtyManager::new());
    assert_eq!(app.on_key(key(KeyCode::Char('q'))), Some(WorkerCmd::Quit));

    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.on_key(ctrl_c), Some(WorkerCmd::Quit));
    // A bare 'c' is not a quit — only ctrl-c is.
    assert_eq!(app.on_key(key(KeyCode::Char('c'))), None);
}

#[test]
fn the_master_cursor_moves_and_clamps() {
    let mut app = app_with(PtyManager::new());
    app.add_master("alice".into(), None);
    app.add_master("bob".into(), None);
    app.set_tab(TAB_MASTER);

    // `add_master` leaves the cursor on the row it just added, so the second
    // one is selected — connecting a master and immediately messaging it should
    // not need a keypress in between.
    assert_eq!(app.selected_master_address().as_deref(), Some("bob"));
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.selected_master_address().as_deref(),
        Some("bob"),
        "clamps at the end"
    );
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.selected_master_address().as_deref(), Some("alice"));
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.selected_master_address().as_deref(),
        Some("alice"),
        "clamps at the start"
    );
}

// -------------------------------------------------------- state accessors ---

#[test]
fn is_headless_reflects_the_chosen_mode() {
    let interactive = app_with(PtyManager::new());
    assert!(!interactive.is_headless(), "interactive is not headless");
    let headless = headless_app(PtyManager::new());
    assert!(headless.is_headless());
    assert_eq!(headless.mode(), Some(ExecutionMode::Headless));
}

#[test]
fn agent_id_and_providers_expose_the_wiring() {
    let app = app_with(PtyManager::new());
    assert_eq!(app.agent_id(), Some("So1anaWa11et"));
    assert_eq!(
        app.providers(),
        &[HarnessProvider::Claude, HarnessProvider::Codex]
    );
}

#[test]
fn with_now_overrides_the_clock() {
    // The event loop injects a clock so time-dependent rendering is testable;
    // the override must actually take effect.
    let app = app_with(PtyManager::new()).with_now(Arc::new(|| 777));
    assert_eq!(app.now(), 777);
}

#[test]
fn the_session_manager_is_exposed_to_the_event_loop() {
    // The event loop reaches the live manager through this accessor to resize
    // and shut down; it must hand back the same manager the app was built with.
    let sessions = PtyManager::new();
    let id = sessions.open(sh("sleep 30", "peer-1")).unwrap();
    let app = app_with(sessions.clone());
    let via_app = app.sessions();
    assert_eq!(
        via_app.rows().len(),
        1,
        "the exposed manager sees the session"
    );
    assert_eq!(via_app.row(&id).unwrap().label, "peer-1");
    sessions.shutdown();
}

#[test]
fn copying_the_address_without_a_capture_sink_uses_the_real_writer() {
    // The capture sink is a test convenience; the production path (a platform
    // clipboard binary, falling back to OSC 52 for the terminal) must also run
    // and report which mechanism took the address. Exercised without a capture
    // so the real writer, not the stub, is walked.
    let mut app = app_with(PtyManager::new());
    app.on_key(key(KeyCode::Char('y')));
    let status = app.status();
    assert!(
        status.contains("Copied") || status.contains("OSC 52") || status.contains("Sent"),
        "the copy must report its mechanism, got {status:?}"
    );
}

#[test]
fn the_session_cursor_moves_and_clamps() {
    let sessions = PtyManager::new();
    sessions.open(sh("sleep 30", "a")).unwrap();
    sessions.open(sh("sleep 30", "b")).unwrap();
    let mut app = app_with(sessions.clone());

    assert_eq!(app.selected_session().unwrap().label, "a");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.selected_session().unwrap().label, "b");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.selected_session().unwrap().label, "b", "clamps");
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.selected_session().unwrap().label, "a");
    sessions.shutdown();
}
