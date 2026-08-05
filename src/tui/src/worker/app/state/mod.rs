//! Construction and state accessors for [`WorkerApp`].

use std::sync::Arc;

use medulla::config::Peer;
use medulla::protocol::HarnessProvider;

use super::super::pty::{PtyManager, SessionRow};
use super::types::{Confirm, ExecutionMode, Screen, SetupStep, WorkerApp, TABS};
use crate::log::LogBuffer;
use crate::ui::theme::Theme;

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
            agent_id: wiring.agent_id,
            providers: wiring.providers,
            tab: 0,
            session_index: 0,
            master_index: 0,
            workspace_index: 0,
            masters: wiring.masters,
            workspaces: normalize_workspaces(wiring.primary_workspace.as_str(), wiring.workspaces),
            primary_workspace: wiring.primary_workspace,
            config_path: wiring.config_path,
            credential_dir: wiring.credential_dir,
            endpoint: wiring.endpoint,
            theme: wiring.theme,
            prompt: None,
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

    /// This daemon's link node name, if it has one.
    pub fn agent_id(&self) -> Option<&str> {
        self.agent_id.as_deref()
    }

    /// The harnesses detected on PATH.
    pub fn providers(&self) -> &[HarnessProvider] {
        &self.providers
    }

    /// Configured master peers.
    pub fn masters(&self) -> &[Peer] {
        &self.masters
    }

    /// The master rows the Master tab renders.
    ///
    /// Identical to [`masters`](Self::masters) today: the link has no contact
    /// graph to merge in, so the configured `[link].peers` roster is the whole
    /// list. Kept as its own accessor because it is what the render layer asks
    /// for, and the two are different questions.
    pub fn master_rows(&self) -> Vec<Peer> {
        self.masters.clone()
    }

    /// Workspace roots currently approved for capability advertisement.
    pub fn workspaces(&self) -> &[String] {
        &self.workspaces
    }

    /// File used for daemon settings.
    pub fn config_path(&self) -> &std::path::Path {
        &self.config_path
    }

    /// Worker-local wallet directory.
    pub fn credential_dir(&self) -> &std::path::Path {
        &self.credential_dir
    }

    /// Add or refresh a resolved master record.
    pub fn add_master(&mut self, address: String, handle: Option<String>) {
        if let Some(existing) = self
            .masters
            .iter_mut()
            .find(|peer| peer.id == address || peer.address.as_deref() == Some(address.as_str()))
        {
            if handle.is_some() {
                existing.handle = handle;
            }
            return;
        }
        self.masters.push(Peer {
            id: address.clone(),
            node_id: None,
            name: Some("Master".into()),
            handle,
            address: Some(address),
            tags: Some(vec!["master".into()]),
            description: Some("Medulla orchestrator controlling this worker".into()),
            protocol: "task".into(),
        });
        self.master_index = self.masters.len().saturating_sub(1);
    }

    /// Selected master's address.
    pub fn selected_master_address(&self) -> Option<String> {
        let rows = self.master_rows();
        rows.get(self.master_index.min(rows.len().saturating_sub(1)))
            .map(|peer| peer.address.clone().unwrap_or_else(|| peer.id.clone()))
    }

    /// Add a canonical workspace if it is not already allowed.
    pub fn add_workspace(&mut self, workspace: String) {
        if !self.workspaces.contains(&workspace) {
            self.workspaces.push(workspace);
            self.workspace_index = self.workspaces.len().saturating_sub(1);
        }
    }

    /// Remove a non-primary workspace from the allowlist.
    pub fn remove_workspace(&mut self, workspace: &str) -> bool {
        if workspace == self.primary_workspace {
            self.set_status("The active workspace is always allowed");
            return false;
        }
        let before = self.workspaces.len();
        self.workspaces.retain(|item| item != workspace);
        self.workspace_index = self
            .workspace_index
            .min(self.workspaces.len().saturating_sub(1));
        self.workspaces.len() != before
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
            self.set_status("No host-link identity yet — nothing to copy");
            return;
        };
        if let Some(sink) = &self.copy_capture {
            sink.lock().expect("copy sink").push(address);
            self.set_status("Copied this worker's address (captured)");
            return;
        }
        // Via the terminal first: a worker is usually a machine reached over
        // SSH, and the operator's clipboard is on the near side of that
        // connection. See [`medulla::clipboard::copy_for_operator`].
        crate::ui::clipboard::copy_for_operator(
            &address,
            crate::ui::clipboard::current_platform(),
            |osc| {
                use std::io::Write;
                let _ = std::io::stdout().write_all(osc.as_bytes());
                let _ = std::io::stdout().flush();
            },
        );
        self.set_status("Copied this worker's address — paste it into Routing › Add Host");
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

/// Keep the active task directory first and deduplicate configured roots.
fn normalize_workspaces(primary: &str, configured: Vec<String>) -> Vec<String> {
    let mut out = vec![primary.to_string()];
    for workspace in configured {
        if !workspace.trim().is_empty() && !out.contains(&workspace) {
            out.push(workspace);
        }
    }
    out
}

mod types;
pub use types::WorkerWiring;
