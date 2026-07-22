//! What the screen shows: the address, the explainers, and the embedded
//! harness terminal.

use crossterm::event::KeyCode;

use super::super::super::pty::PtyManager;
use super::super::types::WorkerApp;
use super::helpers::{app_at_setup, app_with, key, render, sh, wait_for};
use medulla::tinyplace::HarnessProvider;

use super::super::state::WorkerWiring;
use super::super::types::{ExecutionMode, Screen, SetupStep, WorkerCmd, TAB_CONTACTS};

// ----------------------------------------------------------------- render ---

#[test]
fn with_no_identity_the_contact_tabs_explain_rather_than_look_empty() {
    let mut app = app_with(PtyManager::new(), None);
    app.set_tab(TAB_CONTACTS);
    let out = render(&mut app, 110, 16);
    assert!(
        out.contains("No tiny.place identity is configured"),
        "got: {out}"
    );
}

#[test]
fn the_header_shows_this_daemons_address() {
    // A peer needs it to reach this machine, so it is on screen rather than in
    // a startup log line already scrolled past.
    let mut app = app_with(PtyManager::new(), None);
    let out = render(&mut app, 110, 16);
    assert!(out.contains("So1anaWa11et"), "got: {out}");
    assert!(out.contains("MEDULLA WORKER"));
}

#[test]
fn an_empty_fleet_says_what_fills_it() {
    // The operator does not open sessions here — peer work does — so the empty
    // state must not offer a key that no longer exists.
    let mut app = app_with(PtyManager::new(), None);
    let out = render(&mut app, 110, 20);
    assert!(out.contains("No sessions running"));
    assert!(out.contains("Peer tasks open sessions here"), "got: {out}");
}

#[test]
fn a_missing_harness_is_reported_as_such_not_as_an_empty_list() {
    let mut app = WorkerApp::new(WorkerWiring {
        logs: crate::log::LogBuffer::new(),
        sessions: PtyManager::new(),
        contacts: None,
        agent_id: None,
        providers: Vec::new(),
        startup_status: None,
    });
    let out = render(&mut app, 110, 20);
    assert!(out.contains("No coding agents on PATH"), "got: {out}");
}

#[test]
fn a_live_session_renders_its_terminal_in_the_pane() {
    let sessions = PtyManager::new();
    let id = sessions
        .open(sh("echo WORKER-PANE-OK; sleep 30", "peer-1"))
        .unwrap();
    wait_for("output", || {
        sessions
            .screen_rows(&id)
            .map(|s| {
                s.cells.iter().any(|r| {
                    r.iter()
                        .map(|c| c.text.as_str())
                        .collect::<String>()
                        .contains("WORKER-PANE-OK")
                })
            })
            .unwrap_or(false)
    });
    let mut app = app_with(sessions.clone(), None);
    let out = render(&mut app, 110, 20);
    assert!(
        out.contains("WORKER-PANE-OK"),
        "the harness screen must be embedded"
    );
    sessions.shutdown();
}

// ------------------------------------------------------------------ setup ---

#[test]
fn launching_asks_how_the_worker_runs_before_what_it_runs_on() {
    // Mode first: it decides which executor the runtime is built with, and
    // therefore what there is to look at.
    let app = &mut app_at_setup(PtyManager::new(), None);
    assert_eq!(app.screen(), Screen::Setup);
    assert_eq!(app.setup_step(), SetupStep::Mode);
    assert_eq!(app.mode(), None);

    let out = render(app, 90, 20);
    assert!(
        out.contains("How should this worker run the tasks"),
        "got: {out}"
    );
    assert!(out.contains("Headless"));
    assert!(out.contains("Interactive"));
}

#[test]
fn answering_the_mode_advances_to_the_harness_question() {
    let app = &mut app_at_setup(PtyManager::new(), None);
    assert!(
        app.on_key(key(KeyCode::Char('1'))).is_none(),
        "not started yet"
    );

    assert_eq!(app.mode(), Some(ExecutionMode::Headless));
    assert_eq!(app.setup_step(), SetupStep::Harness);
    assert_eq!(app.screen(), Screen::Setup, "still setting up");

    let out = render(app, 90, 20);
    assert!(out.contains("Which coding agent"), "got: {out}");
    // The settled answer stays visible, so the second question is answered in
    // the context of the first.
    assert!(out.contains("running headless"), "got: {out}");
}

#[test]
fn answering_both_questions_starts_the_worker() {
    let app = &mut app_at_setup(PtyManager::new(), None);
    app.on_key(key(KeyCode::Char('2'))); // interactive
    let cmd = app.on_key(key(KeyCode::Char('2'))); // codex

    match cmd {
        Some(WorkerCmd::Start { mode, provider }) => {
            assert_eq!(mode, ExecutionMode::Interactive);
            assert_eq!(provider, HarnessProvider::Codex);
        }
        other => panic!("expected Start, got {other:?}"),
    }
    assert_eq!(app.screen(), Screen::Main);
    let out = render(app, 90, 20);
    assert!(out.contains("interactive on codex"), "header: {out}");
}

#[test]
fn nothing_listens_for_peer_work_until_setup_is_answered() {
    // A worker should not accept work before being told how to run it, so the
    // start command is the only thing that opens the inbox.
    let app = &mut app_at_setup(PtyManager::new(), None);
    for code in [KeyCode::Up, KeyCode::Down, KeyCode::Tab] {
        assert!(
            !matches!(app.on_key(key(code)), Some(WorkerCmd::Start { .. })),
            "{code:?} must not start the worker"
        );
    }
    assert_eq!(app.screen(), Screen::Setup);
}

#[test]
fn a_single_installed_harness_is_settled_without_a_menu_of_one() {
    // One option is an answer, not a question — but the mode still is one.
    let mut app = WorkerApp::new(WorkerWiring {
        logs: crate::log::LogBuffer::new(),
        sessions: PtyManager::new(),
        contacts: None,
        agent_id: None,
        providers: vec![HarnessProvider::Codex],
        startup_status: None,
    });
    assert_eq!(
        app.screen(),
        Screen::Setup,
        "the mode is still worth asking"
    );

    match app.on_key(key(KeyCode::Char('1'))) {
        Some(WorkerCmd::Start { mode, provider }) => {
            assert_eq!(mode, ExecutionMode::Headless);
            assert_eq!(provider, HarnessProvider::Codex, "settled, not asked");
        }
        other => panic!("expected Start, got {other:?}"),
    }
    assert_eq!(app.screen(), Screen::Main);
}

#[test]
fn with_no_harness_installed_there_is_nothing_to_ask_and_the_screen_says_so() {
    let mut app = WorkerApp::new(WorkerWiring {
        logs: crate::log::LogBuffer::new(),
        sessions: PtyManager::new(),
        contacts: None,
        agent_id: None,
        providers: Vec::new(),
        startup_status: None,
    });
    assert_eq!(
        app.screen(),
        Screen::Main,
        "a menu with no options helps nobody"
    );
    assert_eq!(app.harness(), None);
    assert!(render(&mut app, 90, 20).contains("No coding agents on PATH"));
}

#[test]
fn the_setup_step_can_be_quit_without_choosing() {
    let app = &mut app_at_setup(PtyManager::new(), None);
    assert_eq!(app.on_key(key(KeyCode::Char('q'))), Some(WorkerCmd::Quit));
}

// --------------------------------------------------------------- headless ---

#[test]
fn headless_renders_the_daemon_log_instead_of_a_terminal() {
    // There is no screen to embed when every task is a one-shot process; the
    // daemon's own narration is the whole view.
    let mut app = super::helpers::headless_app(PtyManager::new(), None);
    app.logs().push("task t_8f3a → claude");
    app.logs().push("task t_8f3a ✓ (12 events)");

    let out = render(&mut app, 100, 18);
    assert!(out.contains("Daemon log"), "got: {out}");
    assert!(out.contains("task t_8f3a → claude"));
    assert!(out.contains("✓ (12 events)"));
    assert!(
        !out.contains("Terminal"),
        "there is no terminal pane in headless mode"
    );
}

#[test]
fn the_first_tab_is_labelled_for_the_mode() {
    // Calling it "Sessions" in headless would promise something that never
    // appears.
    let mut headless = super::helpers::headless_app(PtyManager::new(), None);
    assert!(render(&mut headless, 100, 14).contains("1 Log"));

    let mut interactive = app_with(PtyManager::new(), None);
    assert!(render(&mut interactive, 100, 14).contains("1 Sessions"));
}

#[test]
fn an_empty_headless_log_explains_what_it_is_waiting_for() {
    let mut app = super::helpers::headless_app(PtyManager::new(), None);
    let out = render(&mut app, 100, 18);
    assert!(out.contains("Waiting for peer work"), "got: {out}");
}

#[test]
fn contacts_and_requests_are_the_same_in_both_modes() {
    // The contact graph is a property of the machine, not of how it runs tasks.
    for mut app in [
        super::helpers::headless_app(PtyManager::new(), None),
        app_with(PtyManager::new(), None),
    ] {
        app.set_tab(TAB_CONTACTS);
        assert!(render(&mut app, 100, 16).contains("Contacts"));
    }
}

// ---------------------------------------------------------- copy address ---

#[test]
fn y_copies_this_workers_address() {
    // The orchestrator addresses a worker by this string, and the screen holds
    // the terminal's mouse capture — so without a key the address on screen can
    // only be retyped.
    let mut app = app_with(PtyManager::new(), None);
    let sink = app.capture_clipboard();

    app.on_key(key(KeyCode::Char('y')));

    assert_eq!(
        sink.lock().unwrap().clone(),
        vec!["So1anaWa11et".to_string()]
    );
    assert!(app.status().contains("Copied"), "got {:?}", app.status());
}

#[test]
fn the_address_can_be_copied_from_any_tab() {
    for tab in [TAB_CONTACTS, super::super::types::TAB_REQUESTS] {
        let mut app = app_with(PtyManager::new(), None);
        let sink = app.capture_clipboard();
        app.set_tab(tab);
        app.on_key(key(KeyCode::Char('y')));
        assert_eq!(sink.lock().unwrap().len(), 1, "tab {tab} could not copy");
    }
}

#[test]
fn copying_without_an_identity_says_so_rather_than_copying_nothing() {
    let mut app = WorkerApp::new(WorkerWiring {
        logs: crate::log::LogBuffer::new(),
        sessions: PtyManager::new(),
        contacts: None,
        agent_id: None,
        providers: vec![HarnessProvider::Codex],
        startup_status: None,
    });
    app.choose_mode(ExecutionMode::Headless);
    app.choose_harness(HarnessProvider::Codex);
    let sink = app.capture_clipboard();

    app.on_key(key(KeyCode::Char('y')));
    assert!(sink.lock().unwrap().is_empty(), "nothing to copy");
    assert!(
        app.status().contains("No tiny.place identity"),
        "got {:?}",
        app.status()
    );
}
