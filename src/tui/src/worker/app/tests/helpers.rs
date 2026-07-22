//! Shared fixtures for the worker-TUI screen tests: the fake contact relay,
//! pty specs over `/bin/sh`, and the app/render helpers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures::future::BoxFuture;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::contacts::{AdmissionPolicy, ContactDesk, ContactRelay, IncomingRequest};
use medulla::tinyplace::HarnessProvider;

use super::super::super::pty::{LaunchSpec, PtyManager};
use super::super::state::WorkerWiring;
use super::super::types::WorkerApp;

// ------------------------------------------------------------------ stubs ---

pub(super) struct FakeRelay {
    incoming: Vec<IncomingRequest>,
    /// Peers the relay already holds as contacts, independent of any request.
    contacts: Vec<IncomingRequest>,
}

impl ContactRelay for FakeRelay {
    fn incoming(&self) -> BoxFuture<'_, Result<Vec<IncomingRequest>, String>> {
        Box::pin(async move { Ok(self.incoming.clone()) })
    }
    fn accepted(&self) -> BoxFuture<'_, Result<Vec<IncomingRequest>, String>> {
        Box::pin(async move { Ok(self.contacts.clone()) })
    }
    fn accept(&self, _: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn decline(&self, _: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn block(&self, _: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

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
        bin: "/bin/sh".to_string(),
        cwd: "/".to_string(),
        env,
        extra_args: vec!["-c".to_string(), script.to_string()],
        skip_permissions: false,
        label: label.to_string(),
        session_id: None,
    }
}

/// A desk whose queue already holds `ids` as pending.
pub(super) async fn desk_with(ids: &[&str]) -> ContactDesk {
    desk_with_contacts(ids, &[]).await
}

/// A desk with `pending` incoming requests and `contacts` the relay already
/// holds as established — the two are independent listings on the relay, and a
/// contact never appears in the request queue.
pub(super) async fn desk_with_contacts(pending: &[&str], contacts: &[&str]) -> ContactDesk {
    let peers = |ids: &[&str]| -> Vec<IncomingRequest> {
        ids.iter()
            .map(|id| IncomingRequest {
                agent_id: (*id).to_string(),
                handle: None,
            })
            .collect()
    };
    let relay = || FakeRelay {
        incoming: peers(pending),
        contacts: peers(contacts),
    };
    let desk = ContactDesk::new(
        Arc::new(relay()),
        AdmissionPolicy::Manual,
        Vec::<String>::new(),
    );
    medulla::contacts::poll_once(
        &relay(),
        desk.book(),
        &(Arc::new(|| 1_000i64) as Arc<dyn Fn() -> i64 + Send + Sync>),
    )
    .await
    .unwrap();
    desk
}

/// An app past the setup step, on the running worker screen.
///
/// Setup now asks two questions — how tasks run, then on what — so both are
/// answered here. Interactive, because that is the mode with sessions to test.
pub(super) fn app_with(sessions: PtyManager, contacts: Option<ContactDesk>) -> WorkerApp {
    let mut app = app_at_setup(sessions, contacts);
    app.choose_mode(super::super::types::ExecutionMode::Interactive);
    app.choose_harness(HarnessProvider::Claude);
    app
}

/// An app on the running worker screen in **headless** mode.
pub(super) fn headless_app(sessions: PtyManager, contacts: Option<ContactDesk>) -> WorkerApp {
    let mut app = app_at_setup(sessions, contacts);
    app.choose_mode(super::super::types::ExecutionMode::Headless);
    app.choose_harness(HarnessProvider::Claude);
    app
}

/// An app as it launches, still on the setup step.
pub(super) fn app_at_setup(sessions: PtyManager, contacts: Option<ContactDesk>) -> WorkerApp {
    WorkerApp::new(WorkerWiring {
        logs: crate::log::LogBuffer::new(),
        sessions,
        contacts,
        agent_id: Some("So1anaWa11et".to_string()),
        providers: vec![HarnessProvider::Claude, HarnessProvider::Codex],
        startup_status: None,
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
