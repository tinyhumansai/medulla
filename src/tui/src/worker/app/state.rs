//! Construction and state accessors for [`WorkerApp`].

use std::sync::Arc;

use medulla::contacts::{ContactDesk, ContactRequest, RequestState};
use medulla::tinyplace::HarnessProvider;

use super::super::pty::{PtyManager, SessionRow};
use super::types::{Confirm, ExecutionMode, Screen, SetupStep, WorkerApp, TABS};
use crate::log::LogBuffer;

/// How the worker TUI is wired at startup.
pub struct WorkerWiring {
    /// The session manager (shared with the daemon's inbound path).
    pub sessions: PtyManager,
    /// The contact desk, when a tiny.place identity is configured.
    pub contacts: Option<ContactDesk>,
    /// This daemon's own tiny.place address.
    pub agent_id: Option<String>,
    /// Harnesses detected on PATH.
    pub providers: Vec<HarnessProvider>,
    /// A note to show on the status line at startup.
    pub startup_status: Option<String>,
    /// Where the daemon's log lines are captured.
    pub logs: LogBuffer,
}

impl WorkerApp {
    /// Build the worker TUI from its wiring.
    pub fn new(wiring: WorkerWiring) -> Self {
        let status = wiring.startup_status.unwrap_or_else(|| {
            if wiring.providers.is_empty() {
                "No coding agents found on PATH — install claude or codex".to_string()
            } else {
                format!(
                    "Ready · {} available",
                    wiring
                        .providers
                        .iter()
                        .map(|p| p.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        });
        // Nothing to ask when there is no choice: one harness installed is the
        // answer, and none installed is a problem the main screen states
        // plainly rather than a menu with no options.
        // The mode question is always worth asking, so setup always runs when a
        // harness exists at all. With none installed there is nothing to
        // configure and the main screen states the problem instead.
        let (screen, harness) = match wiring.providers.as_slice() {
            [] => (Screen::Main, None),
            // One harness is an answer, not a question — but the mode still is.
            [only] => (Screen::Setup, Some(*only)),
            _ => (Screen::Setup, None),
        };
        WorkerApp {
            screen,
            setup_step: SetupStep::Mode,
            setup_index: 0,
            mode: None,
            logs: wiring.logs,
            log_scroll: 0,
            harness,
            sessions: wiring.sessions,
            contacts: wiring.contacts,
            agent_id: wiring.agent_id,
            providers: wiring.providers,
            tab: 0,
            session_index: 0,
            contact_index: 0,
            request_index: 0,
            confirm: None,
            status,
            should_quit: false,
            mouse_capture: true,
            hit_tabs: (0, Vec::new()),
            hit_rows: None,
            hit_setup: None,
            terminal_area: ratatui::layout::Rect::new(0, 0, 0, 0),
            now: Arc::new(medulla::clock::now_millis),
            copy_capture: None,
        }
    }

    /// Override the clock (tests).
    pub fn with_now(mut self, now: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.now = now;
        self
    }

    /// The current clock reading, in epoch ms.
    pub(super) fn now(&self) -> i64 {
        (self.now)()
    }

    /// Jump to a tab by index, clamped.
    pub fn set_tab(&mut self, index: usize) {
        self.tab = index.min(TABS.len() - 1);
    }

    /// Every live session, open order.
    pub fn session_rows(&self) -> Vec<SessionRow> {
        self.sessions.rows()
    }

    /// The session under the list cursor.
    pub fn selected_session(&self) -> Option<SessionRow> {
        let rows = self.session_rows();
        if rows.is_empty() {
            return None;
        }
        rows.get(self.session_index.min(rows.len() - 1)).cloned()
    }

    /// Every known contact request, first-seen order.
    pub fn requests(&self) -> Vec<ContactRequest> {
        self.contacts
            .as_ref()
            .map(|desk| desk.requests())
            .unwrap_or_default()
    }

    /// Only the requests still waiting on a decision — the Requests tab's rows
    /// and the tab-bar badge.
    pub fn pending_requests(&self) -> Vec<ContactRequest> {
        self.requests()
            .into_iter()
            .filter(|r| r.state == RequestState::Pending)
            .collect()
    }

    /// The peers that have been accepted — the Contacts tab's rows.
    ///
    /// Asked of the desk rather than filtered out of the request queue: an
    /// accepted contact stops being a pending request, so filtering the queue
    /// showed only the peers this process accepted while it was running — an
    /// empty list on every restart.
    pub fn accepted_contacts(&self) -> Vec<ContactRequest> {
        self.contacts
            .as_ref()
            .map(|desk| desk.accepted())
            .unwrap_or_default()
    }

    /// The request under the Requests-tab cursor.
    pub fn selected_request(&self) -> Option<ContactRequest> {
        let rows = self.pending_requests();
        if rows.is_empty() {
            return None;
        }
        rows.get(self.request_index.min(rows.len() - 1)).cloned()
    }

    /// The contact under the Contacts-tab cursor.
    pub fn selected_contact(&self) -> Option<ContactRequest> {
        let rows = self.accepted_contacts();
        if rows.is_empty() {
            return None;
        }
        rows.get(self.contact_index.min(rows.len() - 1)).cloned()
    }

    /// The contact desk, for the event loop's async decisions.
    pub fn contact_desk(&self) -> Option<ContactDesk> {
        self.contacts.clone()
    }

    /// This daemon's tiny.place address, if it has one.
    pub fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }

    /// The harnesses detected on PATH.
    pub fn providers(&self) -> &[HarnessProvider] {
        &self.providers
    }

    /// Answer the first setup question and move to the second.
    pub fn choose_mode(&mut self, mode: ExecutionMode) {
        self.mode = Some(mode);
        self.setup_step = SetupStep::Harness;
        self.setup_index = 0;
    }

    /// Answer the second setup question and enter the running worker.
    ///
    /// Nothing has been listening to the network until now: a worker should not
    /// accept peer work before the operator has said how it should run it.
    pub fn choose_harness(&mut self, provider: HarnessProvider) {
        self.harness = Some(provider);
        self.screen = Screen::Main;
        let mode = self.mode.unwrap_or(ExecutionMode::Headless);
        self.set_status(format!(
            "Serving peers · {} on {}",
            mode.as_str(),
            provider.as_str()
        ));
    }

    /// Route copying into a captured sink instead of the OS clipboard (tests).
    pub fn capture_clipboard(&mut self) -> Arc<std::sync::Mutex<Vec<String>>> {
        let sink = Arc::new(std::sync::Mutex::new(Vec::new()));
        self.copy_capture = Some(sink.clone());
        sink
    }

    /// Copy this worker's address, so it can be handed to an orchestrator.
    ///
    /// This remains the fastest way to copy the identity while mouse capture is
    /// active; Ctrl-O releases capture when arbitrary on-screen text is needed.
    pub fn copy_address(&mut self) {
        let Some(address) = self.agent_id.clone() else {
            self.set_status("No tiny.place identity yet — nothing to copy");
            return;
        };
        if let Some(sink) = &self.copy_capture {
            sink.lock().expect("copy sink").push(address);
            self.set_status("Copied this worker's address (captured)");
            return;
        }
        let via = crate::ui::clipboard::copy_to_clipboard(
            &address,
            crate::ui::clipboard::current_platform(),
            |osc| {
                use std::io::Write;
                let _ = std::io::stdout().write_all(osc.as_bytes());
                let _ = std::io::stdout().flush();
            },
        );
        self.set_status(if via == crate::ui::clipboard::OSC_52 {
            "Sent this worker's address to the terminal (OSC 52) — check your clipboard"
        } else {
            "Copied this worker's address — add it as a worker in the orchestrator"
        });
    }

    /// Whether this worker runs tasks headlessly.
    pub fn is_headless(&self) -> bool {
        self.mode == Some(ExecutionMode::Headless)
    }

    /// Arm a destructive confirmation.
    pub(super) fn arm(&mut self, confirm: Confirm) {
        self.set_status(confirm.prompt());
        self.confirm = Some(confirm);
    }

    /// Discard a pending confirmation.
    pub(super) fn disarm(&mut self) {
        self.confirm = None;
    }
}
