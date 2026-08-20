//! Pointer ownership while an operator-owned harness holds the keyboard.
//!
//! These tests cover terminal pointer grabs and clicks on the attached session's
//! own rail row without relying on the retired session-handoff UI.
//!
//! The child is a real `/bin/sh` on a real pty that turns mouse reporting on and
//! echoes what it receives with `cat -v`, so the assertions read the actual
//! bytes that reached it rather than a record of what we meant to send.
//!
//! Unix-only: it runs `/bin/sh` on a pty, which Windows has no equivalent of.

#![cfg(unix)]

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

/// The terminal the assertions are written against.
///
/// Wide enough that the rail and the pane are both comfortably sized, and tall
/// enough that two harness rows are on screen at once — the second row is what
/// the rail half of this file clicks on.
const WIDTH: u16 = 140;
/// Rows of that terminal.
const HEIGHT: u16 = 44;

/// An Agents-tab app with a live session manager behind it.
fn app_with_harnesses(sessions: PtyManager) -> App {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.tab_index = TABS.iter().position(|t| *t == "Sessions").unwrap();

    let config = medulla::daemon::DaemonConfig {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        providers: vec![HarnessProvider::Codex],
        default_provider: HarnessProvider::Codex,
        workspace: "/".to_string(),
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
        Arc::new(|_| Box::pin(async { Err("these tests dispatch nothing".to_string()) }));
    let send: medulla::daemon::SendFn = Arc::new(|_, _| {
        Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    });

    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());

    app.set_local_sessions(LocalSessions {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        log: None,
        sessions,
        runtimes: std::sync::Arc::new(std::sync::Mutex::new(vec![
            medulla::daemon::DaemonRuntime::new(config, run_task, send),
        ])),
        hub_address: "medulla-orchestrator".to_string(),
        env,
        workspace: "/".to_string(),
        providers: vec![HarnessProvider::Codex],
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
    });
    app
}

/// A harness the operator holds, whose child asks for the mouse and echoes it.
///
/// `stty raw -echo` first so the pty stops rewriting what arrives; the DECSET
/// pair is what makes [`LocalSessions::takes_mouse`] true; `cat -v` renders the
/// reports it receives as printable text, which is what the pane snapshot can
/// then be searched for.
fn mouse_reporting_session(sessions: &PtyManager) -> String {
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
            extra_args: vec![
                "-c".to_string(),
                "stty raw -echo; printf 'READY\\r\\n\\033[?1000h\\033[?1006h'; cat -v".to_string(),
            ],
            skip_permissions: false,
            label: "you:codex".to_string(),
            model: None,
            session_id: None,
            control: SessionControl::User,
            origin: medulla_tui::worker::pty::SessionOrigin::User,
            name: None,
            mcp_grant_session: None,
        })
        .expect("open a session")
}

/// The harness's whole screen as one string.
fn screen(harnesses: &LocalSessions, id: &str) -> String {
    harnesses
        .screen(id)
        .map(|snapshot| {
            snapshot
                .cells
                .iter()
                .map(|row| row.iter().map(|c| c.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Spin until `check` passes; children on real ptys are at the mercy of load.
fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

/// Draw a frame and return its rows, so a test can find the row it means to
/// click rather than hardcoding a layout it does not control.
fn render(app: &mut App) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..HEIGHT)
        .map(|y| (0..WIDTH).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect()
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn pointer(kind: MouseEventKind, column: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

fn press(column: u16, row: u16) -> Event {
    pointer(MouseEventKind::Down(MouseButton::Left), column, row)
}

fn release(column: u16, row: u16) -> Event {
    pointer(MouseEventKind::Up(MouseButton::Left), column, row)
}

/// Walk the rail to the first harness row and take the keyboard into it.
///
/// Returns the session the pane resolved. The cursor is moved with the arrows
/// and a frame is drawn after each step because `pane_session` is
/// written during the draw — this is the same sequence an operator performs, and
/// the only one that leaves the pane rect recorded for the pointer to use.
fn attach_to_first_harness(app: &mut App) -> String {
    render(app);
    // Esc takes focus off the composer and puts it on the rail.
    let _ = app.on_event(key(KeyCode::Esc));
    render(app);
    for _ in 0..16 {
        if app.pane_session_for_test().is_some() {
            break;
        }
        let _ = app.on_event(key(KeyCode::Down));
        render(app);
    }
    let session = app
        .pane_session_for_test()
        .expect("the rail cursor to reach a harness row")
        .to_string();
    let _ = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char(']'),
        KeyModifiers::CONTROL,
    )));
    render(app);
    assert_eq!(
        app.attached_session(),
        Some(session.as_str()),
        "the chord must attach before the pointer cases begin"
    );
    session
}

/// The rows a harness occupies on the rail, top to bottom.
fn harness_rail_rows(lines: &[String]) -> Vec<u16> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("codex · unmanaged"))
        .map(|(index, _)| index as u16)
        .collect()
}

#[test]
fn a_release_outside_the_pane_still_reaches_the_harness_that_took_the_press() {
    // A terminal grabs the pointer on press. Without that, dragging out of the
    // pane and letting go left the child holding a button nobody was pressing,
    // and every later motion read as a drag from a stale anchor — which is what
    // put its popups somewhere the operator had not clicked.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = mouse_reporting_session(&sessions);
    let harnesses = app.local_sessions().expect("harnesses").clone();
    wait_for("the child to paint", || {
        screen(&harnesses, &id).contains("READY")
    });
    wait_for("the child to ask for the mouse", || {
        harnesses.takes_mouse(&id)
    });

    attach_to_first_harness(&mut app);
    let (pane, _) = app
        .harness_pane_rect_for_test()
        .expect("the draw to record the pane");

    let _ = app.on_event(press(pane.x + 2, pane.y + 2));
    wait_for("the press to arrive", || {
        screen(&harnesses, &id).contains("^[[<0;3;3M")
    });

    // Let go over the rail, well outside the pane.
    let _ = app.on_event(release(1, 1));
    wait_for("the release to arrive", || {
        screen(&harnesses, &id).contains("^[[<0;1;1m")
    });

    let out = screen(&harnesses, &id);
    assert!(
        out.contains("^[[<0;3;3M^[[<0;1;1m"),
        "the release must follow its own press, clamped into the pane: {out}"
    );

    sessions.shutdown();
}

#[test]
fn clicking_the_attached_session_own_row_neither_asks_nor_releases() {
    // The row you are already typing in is not a destination away from it. This
    // is the carve-out the rail rule above narrows to, and it has to survive:
    // raising a handover question over a pane the operator is mid-sentence in
    // leaves them with nothing useful to press but Esc.
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = mouse_reporting_session(&sessions);
    let harnesses = app.local_sessions().expect("harnesses").clone();
    wait_for("the child to paint", || {
        screen(&harnesses, &id).contains("READY")
    });

    let attached = attach_to_first_harness(&mut app);
    let lines = render(&mut app);
    let rows = harness_rail_rows(&lines);

    let _ = app.on_event(press(5, rows[0]));
    let out = render(&mut app).join("\n");
    assert!(
        !out.contains("You still have this session"),
        "clicking your own row must ask nothing: {out}"
    );
    assert_eq!(
        app.attached_session(),
        Some(attached.as_str()),
        "and must leave the keyboard where it was"
    );
    assert_eq!(
        harnesses.control(&attached),
        Some(SessionControl::User),
        "and must not change who holds the harness"
    );

    sessions.shutdown();
}
