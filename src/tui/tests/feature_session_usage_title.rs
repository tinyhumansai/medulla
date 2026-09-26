//! The session pane's title reports what that harness is costing the machine.
//!
//! The line used to read `codex · running · you` — three facts the rail beside
//! it already carries, spent on the pane's most-read row. What is not anywhere
//! else on screen is the cost of the agent the operator is watching, so that is
//! what the title says now, sampled from the child's own process tree.
//!
//! A real `/bin/sh` on a real pty stands in for a harness: the pid the reading
//! is taken from has to be a process the kernel will actually answer for, which
//! is the whole thing under test.
//!
//! Unix-only: it runs `/bin/sh` on a pty, which Windows has no equivalent of.

#![cfg(unix)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::LoadedConfig;
use medulla::protocol::HarnessProvider;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla_tui::ui::app::{App, TABS};
use medulla_tui::ui::harness_pane::LocalSessions;
use medulla_tui::worker::pty::{LaunchSpec, PtyManager, SessionControl};

/// Wide enough that the title is not truncated before its segments are drawn.
const WIDTH: u16 = 160;
/// Rows of that terminal.
const HEIGHT: u16 = 44;

/// An Agents-tab app with a live session manager behind it.
fn app_with_harnesses(sessions: PtyManager) -> App {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.tab_index = TABS.iter().position(|t| *t == "Sessions").unwrap();
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    app.set_local_sessions(LocalSessions {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        log: None,
        sessions,
        runtimes: Arc::new(std::sync::Mutex::new(Vec::new())),
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

/// A harness the operator holds, whose child simply stays alive.
fn idle_session(sessions: &PtyManager) -> String {
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
                "printf 'READY\\r\\n'; sleep 30".to_string(),
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

/// Draw a frame and return its rows.
fn render(app: &mut App) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..HEIGHT)
        .map(|y| (0..WIDTH).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect()
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

/// Walk the rail until the pane resolves a harness, drawing between steps
/// because the pane's session is written during the draw.
fn select_first_harness(app: &mut App) -> Vec<String> {
    render(app);
    let _ = app.on_event(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    let mut lines = render(app);
    for _ in 0..16 {
        if app.pane_session_for_test().is_some() {
            break;
        }
        let _ = app.on_event(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
        lines = render(app);
    }
    app.pane_session_for_test()
        .expect("the rail cursor to reach a harness row");
    lines
}

/// The pane's title line — the one carrying the way in, which is drawn on the
/// harness block's top border.
fn title(lines: &[String]) -> String {
    lines
        .iter()
        .find(|line| line.contains("to type") || line.contains("typing here"))
        .expect("the harness pane draws a title")
        .to_string()
}

#[test]
fn the_pane_title_reports_the_session_s_own_cpu_memory_and_io() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());
    let id = idle_session(&sessions);
    wait_for("the child to paint", || {
        sessions.row(&id).is_some_and(|row| row.state.is_running())
    });

    let title = title(&select_first_harness(&mut app));

    assert!(title.contains("CPU "), "{title}");
    assert!(title.contains("RAM "), "{title}");
    assert!(title.contains("IO "), "{title}");
    // The facts the rail already carries are gone from the title: this line is
    // the one place the operator can learn what the agent costs, and it is only
    // worth that if it is not also repeating what is beside it.
    assert!(!title.contains("running"), "{title}");
    // The way in is still there. A resource readout that displaced the chord
    // would have made the pane unenterable in practice.
    assert!(title.contains("to type"), "{title}");

    sessions.close(&id);
}

#[test]
fn a_session_that_has_ended_names_no_process_to_sample() {
    // What keeps a dead pane from reporting somebody else's usage. The pid a
    // reaped child left behind is a number the kernel is free to hand to the
    // next process that asks, so the sampler must never be pointed at one — and
    // the title falls back to the session's state instead.
    let sessions = PtyManager::new();
    let id = idle_session(&sessions);
    wait_for("the child to start", || {
        sessions.row(&id).is_some_and(|row| row.state.is_running())
    });
    assert!(
        sessions.pid(&id).is_some(),
        "a live session names its child"
    );

    sessions.close(&id);
    wait_for("the child to go", || {
        sessions.row(&id).is_some_and(|row| !row.state.is_running())
    });

    assert_eq!(sessions.pid(&id), None);
}
