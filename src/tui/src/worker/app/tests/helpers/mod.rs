//! Shared fixtures for the worker-TUI screen tests: pty specs over `/bin/sh`
//! and the app/render helpers.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::protocol::HarnessProvider;

use super::super::super::pty::{LaunchSpec, PtyManager, SessionControl};
use super::super::state::WorkerWiring;
use super::super::types::WorkerApp;

/// A spec that runs `sh -c <script>` on a pty.
pub(super) fn sh(script: &str, label: &str) -> LaunchSpec {
    let mut env = HashMap::new();
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    LaunchSpec {
        // Codex takes no preset session id, so its interactive argv is empty
        // and `/bin/sh` receives only the script.
        provider: HarnessProvider::Codex,
        preset: None,
        bin: "/bin/sh".to_string(),
        cwd: "/".to_string(),
        env,
        extra_args: vec!["-c".to_string(), script.to_string()],
        skip_permissions: false,
        label: label.to_string(),
        session_id: None,
        model: None,
        control: SessionControl::Orchestrator,
        origin: crate::worker::pty::SessionOrigin::Orchestrator,
        name: None,
        mcp_grant_session: None,
    }
}

/// An app past the setup step, on the running worker screen.
///
/// Setup now asks two questions — how tasks run, then on what — so both are
/// answered here. Interactive, because that is the mode with sessions to test.
pub(super) fn app_with(sessions: PtyManager) -> WorkerApp {
    let mut app = app_at_setup(sessions);
    app.choose_mode(super::super::types::ExecutionMode::Interactive);
    app.choose_harness(HarnessProvider::Claude);
    app
}

/// An app on the running worker screen in **headless** mode.
pub(super) fn headless_app(sessions: PtyManager) -> WorkerApp {
    let mut app = app_at_setup(sessions);
    app.choose_mode(super::super::types::ExecutionMode::Headless);
    app.choose_harness(HarnessProvider::Claude);
    app
}

/// An app as it launches, still on the setup step.
pub(super) fn app_at_setup(sessions: PtyManager) -> WorkerApp {
    WorkerApp::new(WorkerWiring {
        logs: crate::log::LogBuffer::new(),
        sessions,
        agent_id: Some("So1anaWa11et".to_string()),
        providers: vec![HarnessProvider::Claude, HarnessProvider::Codex],
        startup_status: None,
        primary_workspace: "/workspace".into(),
        workspaces: vec!["/workspace".into()],
        masters: Vec::new(),
        config_path: "/tmp/medulla-test-config.toml".into(),
        credential_dir: "/tmp/medulla-test-wallet".into(),
        endpoint: Some("https://relay.test".into()),
        theme: crate::ui::theme::Theme::default(),
    })
}

pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(super) fn render(app: &mut WorkerApp, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect()
}

/// Draw and return one string per terminal row so narrow-layout tests can
/// distinguish clipping from wrapping.
pub(super) fn render_lines(app: &mut WorkerApp, w: u16, h: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(w as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect()
}

pub(super) fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out after 30s waiting for: {what}");
}

mod types;
