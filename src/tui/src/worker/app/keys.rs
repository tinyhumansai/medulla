//! Keyboard handling for the worker TUI.
//!
//! The worker TUI is an **observer**, not a terminal multiplexer. It shows the
//! peer-driven claude/codex sessions as they run, and lets the operator manage
//! contacts and requests — but it never types into a session and never opens
//! one. Peer work is what opens sessions; the operator watches. So the TUI owns
//! the keyboard at all times, and there is no attach, no detach chord, and no
//! key-forwarding: every key drives the chrome.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use medulla::contacts::ContactDecision;

use super::types::{
    Confirm, ExecutionMode, Screen, SetupStep, WorkerApp, WorkerCmd, EXECUTION_MODES, TABS,
    TAB_CONTACTS, TAB_REQUESTS, TAB_SESSIONS,
};

impl WorkerApp {
    /// Handle one key press, producing any follow-up command.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<WorkerCmd> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // Mouse reporting and native terminal selection are mutually exclusive.
        // Keep the release chord reachable from setup and confirmation screens.
        if ctrl && key.code == KeyCode::Char('o') {
            self.toggle_mouse();
            return None;
        }

        // The launch step owns the keyboard until a harness is chosen.
        if self.screen == Screen::Setup {
            return self.on_setup_key(key, ctrl);
        }

        // A pending destructive confirmation owns the keyboard.
        if let Some(confirm) = self.confirm.clone() {
            return self.on_confirm_key(key, confirm);
        }

        match key.code {
            KeyCode::Char('c') if ctrl => return Some(WorkerCmd::Quit),
            KeyCode::Char('q') => return Some(WorkerCmd::Quit),
            // Available from every tab: the address is what an orchestrator
            // needs, and hunting for the right tab to copy it would be silly.
            KeyCode::Char('y') => self.copy_address(),
            // Waiting out a background interval to learn whether anything is
            // arriving is not much of an answer.
            KeyCode::Char('r') => {
                self.set_status("Checking the relay…");
                return Some(WorkerCmd::Refresh);
            }

            KeyCode::Tab => self.set_tab((self.tab + 1) % TABS.len()),
            KeyCode::BackTab => self.set_tab((self.tab + TABS.len() - 1) % TABS.len()),
            KeyCode::Char(c @ '1'..='3') => {
                self.set_tab(c as usize - '1' as usize);
            }

            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(true),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(false),

            _ => return self.on_tab_key(key),
        }
        None
    }

    /// The launch step: how this worker runs, then what it runs on.
    fn on_setup_key(&mut self, key: KeyEvent, ctrl: bool) -> Option<WorkerCmd> {
        let options = match self.setup_step {
            SetupStep::Mode => EXECUTION_MODES.len(),
            SetupStep::Harness => self.providers.len(),
        };
        let last = options.saturating_sub(1);
        match key.code {
            KeyCode::Char('c') if ctrl => return Some(WorkerCmd::Quit),
            KeyCode::Char('q') | KeyCode::Esc => return Some(WorkerCmd::Quit),
            KeyCode::Up | KeyCode::Char('k') => {
                self.setup_index = self.setup_index.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.setup_index = (self.setup_index + 1).min(last);
            }
            // Number keys answer in one keystroke rather than arrow-then-Enter.
            KeyCode::Char(c @ '1'..='9') => {
                return self.answer_setup(c as usize - '1' as usize);
            }
            KeyCode::Enter => return self.answer_setup(self.setup_index),
            _ => {}
        }
        None
    }

    /// Answer the current setup question with the option at `index`.
    ///
    /// Returns [`WorkerCmd::Start`] once both are answered — that is the moment
    /// the worker begins listening for peer work.
    pub(super) fn answer_setup(&mut self, index: usize) -> Option<WorkerCmd> {
        match self.setup_step {
            SetupStep::Mode => {
                let mode = *EXECUTION_MODES.get(index)?;
                self.choose_mode(mode);
                // One installed harness is an answer, not a question: settle it
                // now rather than showing a menu of one.
                if let [only] = self.providers.as_slice() {
                    let provider = *only;
                    self.choose_harness(provider);
                    return Some(WorkerCmd::Start { mode, provider });
                }
                None
            }
            SetupStep::Harness => {
                let provider = self.providers.get(index).copied()?;
                let mode = self.mode.unwrap_or(ExecutionMode::Headless);
                self.choose_harness(provider);
                Some(WorkerCmd::Start { mode, provider })
            }
        }
    }

    /// Keys specific to the active tab.
    fn on_tab_key(&mut self, key: KeyEvent) -> Option<WorkerCmd> {
        match self.tab {
            TAB_SESSIONS => self.on_sessions_key(key),
            TAB_REQUESTS => self.on_requests_key(key),
            TAB_CONTACTS => self.on_contacts_key(key),
            _ => None,
        }
    }

    /// Sessions tab: kill or drop the selected peer session.
    ///
    /// Watch-only otherwise — the selected session's live screen is already
    /// shown beside the list, so there is nothing to "attach" to. The operator
    /// does not type into a peer's session or open sessions of their own; peer
    /// work is what puts sessions here.
    fn on_sessions_key(&mut self, key: KeyEvent) -> Option<WorkerCmd> {
        match key.code {
            KeyCode::Char('K') => {
                match self.selected_session() {
                    // Killing a harness can lose work in progress, so it asks.
                    Some(row) if row.state.is_running() => {
                        self.arm(Confirm::CloseSession(row.id));
                    }
                    Some(row) => self.set_status(format!("{} has already exited", row.label)),
                    None => self.set_status("No session selected"),
                }
                None
            }
            KeyCode::Char('d') => {
                match self.selected_session() {
                    Some(row) if !row.state.is_running() => {
                        let label = row.label.clone();
                        if self.sessions.forget(&row.id) {
                            self.session_index = self.session_index.saturating_sub(1);
                            self.set_status(format!("Dropped {label}"));
                        }
                    }
                    Some(_) => self.set_status("Still running — press K to kill it first"),
                    None => self.set_status("No session selected"),
                }
                None
            }
            _ => None,
        }
    }

    /// Requests tab: accept, decline, block, cycle policy.
    fn on_requests_key(&mut self, key: KeyEvent) -> Option<WorkerCmd> {
        // Policy is a property of the desk, not of any one request, so it stays
        // reachable when the queue is empty — which is exactly when an operator
        // wants to change it, since the policy is why nothing is queued.
        if key.code == KeyCode::Char('p') {
            self.cycle_policy();
            return None;
        }
        let Some(request) = self.selected_request() else {
            if matches!(key.code, KeyCode::Char('a' | 'x' | 'B')) {
                self.set_status("No pending request selected");
            }
            return None;
        };
        match key.code {
            KeyCode::Char('a') | KeyCode::Enter => Some(WorkerCmd::ContactOp {
                agent_id: request.agent_id,
                decision: ContactDecision::Accept,
            }),
            KeyCode::Char('x') => Some(WorkerCmd::ContactOp {
                agent_id: request.agent_id,
                decision: ContactDecision::Decline,
            }),
            // Blocking is the one decision here that is not casually undone.
            KeyCode::Char('B') => {
                self.arm(Confirm::BlockPeer(request.agent_id));
                None
            }
            _ => None,
        }
    }

    /// Contacts tab: policy only — an accepted peer is revoked from Requests.
    fn on_contacts_key(&mut self, key: KeyEvent) -> Option<WorkerCmd> {
        if key.code == KeyCode::Char('p') {
            self.cycle_policy();
        }
        None
    }

    /// Answer a pending destructive confirmation.
    fn on_confirm_key(&mut self, key: KeyEvent, confirm: Confirm) -> Option<WorkerCmd> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.disarm();
                match confirm {
                    Confirm::CloseSession(id) => {
                        if self.sessions.close(&id) {
                            self.set_status(format!("Killed {id}"));
                        }
                        None
                    }
                    Confirm::BlockPeer(agent_id) => Some(WorkerCmd::ContactOp {
                        agent_id,
                        decision: ContactDecision::Block,
                    }),
                }
            }
            // Anything else declines: a destructive action should need a
            // deliberate "yes", not merely "not Escape".
            _ => {
                self.disarm();
                self.set_status("Cancelled");
                None
            }
        }
    }

    /// Cycle the contact admission policy.
    fn cycle_policy(&mut self) {
        let Some(desk) = &self.contacts else {
            self.set_status("No tiny.place identity — contact policy is unavailable");
            return;
        };
        let policy = desk.cycle_policy();
        self.set_status(format!(
            "Admission → {} ({})",
            policy.as_str(),
            match policy {
                medulla::contacts::AdmissionPolicy::Manual => "every request waits for you",
                medulla::contacts::AdmissionPolicy::Allowlist =>
                    "configured peers admitted, the rest queued",
                medulla::contacts::AdmissionPolicy::All => "every peer admitted automatically",
            }
        ));
    }

    /// Move the active tab's list cursor.
    pub(super) fn move_cursor(&mut self, up: bool) {
        let (index, len) = match self.tab {
            TAB_SESSIONS => (&mut self.session_index, self.sessions.rows().len()),
            TAB_CONTACTS => {
                let len = self.accepted_contacts().len();
                (&mut self.contact_index, len)
            }
            _ => {
                let len = self.pending_requests().len();
                (&mut self.request_index, len)
            }
        };
        let max = len.saturating_sub(1);
        *index = if up {
            index.saturating_sub(1)
        } else {
            (*index + 1).min(max)
        };
    }
}
