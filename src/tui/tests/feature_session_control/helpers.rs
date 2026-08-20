//! Shared fixtures for the session-control binary: an Agents-tab `App` with a
//! live session manager behind it, the event constructors the tests drive it
//! with, and the frame helpers they assert against.
//!
//! Split out under the repo's 500-line ceiling; the behaviour groups that use
//! these live in the sibling modules.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::LoadedConfig;
use medulla::protocol::HarnessProvider;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla_tui::ui::app::{App, TABS};
use medulla_tui::ui::harness_pane::LocalSessions;
use medulla_tui::worker::pty::{LaunchSpec, PtyManager, SessionControl};

/// An Agents-tab app with a live session manager behind it.
///
/// `providers` is Codex alone: its interactive argv is empty, so the picker can
/// launch `/bin/sh` through the ordinary path without claude's minted
/// `--session-id` reaching a shell that would reject it.
pub fn app_with_harnesses(sessions: PtyManager) -> App {
    app_with_workspace(sessions, "/")
}

/// Variant whose picker and shell child use an isolated workspace.
pub fn app_with_workspace(sessions: PtyManager, workspace: &str) -> App {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.tab_index = TABS.iter().position(|t| *t == "Sessions").unwrap();

    let config = medulla::daemon::DaemonConfig {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        providers: vec![HarnessProvider::Codex],
        default_provider: HarnessProvider::Codex,
        workspace: workspace.to_string(),
        accessible_dirs: Vec::new(),
        env: HashMap::new(),
        task_timeout_ms: 1_000,
        capability_timeout_ms: None,
        concurrency: 1,
        status_throttle_ms: 1_000,
        max_pending: 1,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        router: None,
        attribution: true,
        budget: None,
        custom_harnesses: Vec::new(),
    };
    let run_task: medulla::daemon::providers::RunTaskFn =
        Arc::new(|_| Box::pin(async { Err("not used in these tests".to_string()) }));
    let send: medulla::daemon::SendFn = Arc::new(|_, _| {
        Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    });

    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    // The picker resolves the binary through `provider_bin`, which reads this
    // override — so "start codex" starts a shell that sits there instead.
    env.insert("MEDULLA_CODEX_BIN".to_string(), "/bin/sh".to_string());

    app.set_local_sessions(LocalSessions {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        log: None,
        sessions,
        runtimes: std::sync::Arc::new(std::sync::Mutex::new(vec![
            medulla::daemon::DaemonRuntime::new(config, run_task, send),
        ])),
        hub_address: "medulla-orchestrator".to_string(),
        env,
        workspace: workspace.to_string(),
        providers: vec![HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
    });
    app
}

pub fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

pub fn ctrl(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

/// The focus chord, as a terminal actually delivers it.
pub fn focus_chord() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL))
}

pub fn click(column: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

pub fn type_str(app: &mut App, text: &str) {
    for ch in text.chars() {
        let _ = app.on_event(key(KeyCode::Char(ch)));
    }
}

pub fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

/// The same frame as [`render`], kept as rows so a test can find where a label
/// was drawn and aim the pointer at it.
pub fn render_lines(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect()
}

/// Where `label` was drawn, as the column and row an operator would click.
///
/// Searched in the rendered frame rather than recomputed from the layout, so
/// the tests aim at what is actually on screen.
pub fn label_at(lines: &[String], label: &str) -> (u16, u16) {
    for (row, line) in lines.iter().enumerate() {
        if let Some(byte) = line.find(label) {
            // Cells, not bytes: the frame is full of box-drawing characters.
            let column = line[..byte].chars().count() as u16;
            return (column, row as u16);
        }
    }
    panic!("no {label:?} on screen:\n{}", lines.join("\n"));
}

/// Spin until `check` passes; children on real ptys are at the mercy of load.
pub fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

/// A spec for a session the operator started, opened directly.
pub fn user_session(sessions: &PtyManager) -> String {
    session_with_origin(sessions, medulla_tui::worker::pty::SessionOrigin::User)
}

/// The same session, but created by dispatch rather than by a person.
///
/// Origin is immutable and is what decides whether the orchestrator has a claim
/// on a session, so a test about the release question has to be able to open one
/// of each — the two differ in nothing else.
pub fn orchestrator_session(sessions: &PtyManager) -> String {
    session_with_origin(
        sessions,
        medulla_tui::worker::pty::SessionOrigin::Orchestrator,
    )
}

fn session_with_origin(
    sessions: &PtyManager,
    origin: medulla_tui::worker::pty::SessionOrigin,
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
            label: "you:codex".to_string(),
            model: None,
            session_id: None,
            control: SessionControl::User,
            origin,
            name: None,
            mcp_grant_session: None,
        })
        .expect("open")
}

/// Walk the rail cursor onto the operator's harness row.
///
/// The row sits below every lane, so the count is not fixed — stepping down
/// until the pane resolves a session is what makes this independent of how many
/// lanes the mock runtime happens to produce.
pub fn select_harness_row(app: &mut App) {
    for _ in 0..64 {
        let _ = render(app, 140, 44);
        if app.pane_session_for_test().is_some() {
            return;
        }
        let _ = app.on_event(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)));
    }
    panic!("never reached the harness row");
}
