//! Starting harnesses the operator owns, and moving control between them and
//! the orchestrator.
//!
//! Two features that turn out to be one. "Unmanaged" is not a kind of harness —
//! it is a harness the operator holds, and dispatch skips anything the operator
//! holds. So spawning one, taking one over, and handing one back are three
//! spellings of the same state change, and they live together here.
//!
//! Everything in this module is synchronous. Opening a PTY and setting a flag
//! are both immediate, so none of it goes through [`Cmd`](super::types::Cmd) —
//! the operator gets an answer on the status line in the same keystroke, which
//! is the whole point of a control handover being *explicit*.

use crossterm::event::KeyCode;
use medulla::tinyplace::HarnessProvider;

use crate::worker::pty::HarnessControl;

use crate::ui::composer::Draft;

use super::types::{
    tab_pos, App, HandbackPolicy, HandbackPrompt, HarnessPicker, Prompt, PromptKind,
};

impl App {
    /// Open the "start a harness" picker, or spawn directly when the command
    /// already named a provider.
    ///
    /// `/harness` with no provider opens the picker rather than guessing:
    /// starting the wrong CLI in the operator's workspace is not something they
    /// find out about until it has already done something.
    pub(super) fn start_harness_command(&mut self, provider: Option<&str>, path: Option<&str>) {
        let Some(harnesses) = self.harnesses.clone() else {
            self.set_status("This device is not hosting, so it has no harnesses to start");
            return;
        };
        match provider.and_then(HarnessProvider::from_wire) {
            Some(provider) => {
                let cwd = path.unwrap_or("").to_string();
                self.spawn_harness(provider, &cwd);
            }
            None => {
                if harnesses.providers.is_empty() {
                    self.set_status("No harness CLIs found on this device");
                    return;
                }
                self.harness_picker = Some(HarnessPicker {
                    providers: harnesses.providers.clone(),
                    index: 0,
                    cwd: path
                        .map(str::to_string)
                        .unwrap_or_else(|| harnesses.workspace.clone()),
                });
            }
        }
    }

    /// Open the picker from the keyboard shortcut.
    pub(crate) fn open_harness_picker(&mut self) {
        self.start_harness_command(None, None);
    }

    /// Start a harness the operator owns and move the cursor onto it.
    ///
    /// Selecting the new row matters more than it sounds: a harness that
    /// appears somewhere below the fold, with the pane still showing whatever
    /// was selected before, reads as "nothing happened".
    pub(super) fn spawn_harness(&mut self, provider: HarnessProvider, cwd: &str) {
        let Some(harnesses) = self.harnesses.clone() else {
            self.set_status("This device is not hosting, so it has no harnesses to start");
            return;
        };
        let skip = self.harness_skip_permissions;
        match harnesses.open_unmanaged(provider, cwd, skip) {
            Ok(id) => {
                self.tab_index = tab_pos("Agents");
                self.select_harness_row(&id);
                self.set_status(format!(
                    "Started {} · unmanaged, the orchestrator will not use it",
                    provider.as_str()
                ));
            }
            // Surfaced, never swallowed: a spawn that fails silently leaves the
            // operator waiting for a pane that is never coming.
            Err(err) => self.set_status(format!("Could not start {}: {err}", provider.as_str())),
        }
    }

    /// Put the rail cursor on the row for `session_id`, if it has one.
    fn select_harness_row(&mut self, session_id: &str) {
        if let Some(index) = self
            .rail_rows()
            .iter()
            .position(|row| row.session_id() == Some(session_id))
        {
            self.agent_index = index;
        }
    }

    /// Take the selected harness from the orchestrator.
    pub(crate) fn take_harness_control(&mut self) {
        let Some((harnesses, session)) = self.selected_harness() else {
            return;
        };
        if harnesses.control(&session) == Some(HarnessControl::User) {
            self.set_status("You already have this harness");
            return;
        }
        harnesses.set_control(&session, HarnessControl::User);
        self.set_status("You have this harness · the orchestrator will not dispatch into it");
    }

    /// Give the selected harness back to the orchestrator.
    pub(crate) fn hand_harness_back(&mut self) {
        let Some((harnesses, session)) = self.selected_harness() else {
            return;
        };
        if harnesses.control(&session) == Some(HarnessControl::Orchestrator) {
            self.set_status("The orchestrator already has this harness");
            return;
        }
        harnesses.set_control(&session, HarnessControl::Orchestrator);
        self.set_status("Handed back · the orchestrator may dispatch into it again");
    }

    /// Toggle who holds the selected harness — the `Ctrl-G` shortcut.
    ///
    /// One key for both directions because the rail row and the pane title both
    /// say which way it will go, so a single "grab or give" is less to remember
    /// than two chords that each do nothing half the time.
    pub(crate) fn toggle_harness_control(&mut self) {
        let Some((harnesses, session)) = self.selected_harness() else {
            return;
        };
        match harnesses.control(&session) {
            Some(HarnessControl::User) => self.hand_harness_back(),
            Some(HarnessControl::Orchestrator) => self.take_harness_control(),
            None => self.set_status("That harness is gone"),
        }
    }

    /// The harness the cursor is on, with the handle needed to act on it.
    ///
    /// Refuses with a reason rather than silently doing nothing, for the same
    /// reason the attach chord does: an operator who pressed a key and saw no
    /// change cannot tell "wrong row" from "broken feature".
    fn selected_harness(&mut self) -> Option<(crate::ui::harness_pane::LocalHarnesses, String)> {
        let Some(harnesses) = self.harnesses.clone() else {
            self.set_status("This device is not hosting, so it has no harnesses");
            return None;
        };
        let Some(session) = self.harness_pane_session.clone() else {
            self.set_status("No harness on this row — select one to hand it over");
            return None;
        };
        Some((harnesses, session))
    }

    /// Decide what releasing the keyboard should do, and do it.
    ///
    /// Returns `true` when the release is settled and the caller should detach.
    /// `false` means a prompt is now open and the operator is still attached —
    /// releasing before they answer would move the keyboard out from under the
    /// question being asked about it.
    pub(crate) fn begin_harness_release(&mut self, session: &str) -> bool {
        let held = self
            .harnesses
            .as_ref()
            .and_then(|harnesses| harnesses.control(session))
            == Some(HarnessControl::User);
        if !held {
            return true;
        }
        match self.handback_policy {
            HandbackPolicy::Always => {
                self.hand_back_session(session);
                true
            }
            HandbackPolicy::Never => {
                self.set_status(
                    "Released · you still hold this harness (/handoff to give it back)",
                );
                true
            }
            HandbackPolicy::Ask => {
                self.handback_prompt = Some(HandbackPrompt {
                    session: session.to_string(),
                    took_control: self.harness_took_control,
                });
                false
            }
        }
    }

    /// Hand `session` back without touching the cursor, for the release path.
    pub(super) fn hand_back_session(&mut self, session: &str) {
        if let Some(harnesses) = self.harnesses.clone() {
            harnesses.set_control(session, HarnessControl::Orchestrator);
        }
        self.harness_took_control = false;
        self.set_status("Handed back · the orchestrator may dispatch into it again");
    }
}

impl App {
    /// Route a key while the "start a harness" picker is open.
    ///
    /// `e` edits the directory through the ordinary inline prompt rather than
    /// building an editable field into the modal — one text-editing
    /// implementation is enough, and it is the one the operator already knows
    /// from every other prompt in the app.
    pub(super) fn handle_harness_picker_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.harness_picker = None;
                self.set_status("Cancelled");
            }
            KeyCode::Up => {
                if let Some(picker) = &mut self.harness_picker {
                    picker.index = picker.index.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(picker) = &mut self.harness_picker {
                    picker.index = (picker.index + 1).min(picker.providers.len().saturating_sub(1));
                }
            }
            KeyCode::Char('e') => {
                if let Some(picker) = &self.harness_picker {
                    let cursor = picker.cwd.chars().count();
                    self.prompt = Some(Prompt {
                        kind: PromptKind::HarnessCwd,
                        title: "Start the harness in — directory".into(),
                        draft: Draft {
                            text: picker.cwd.clone(),
                            cursor,
                        },
                    });
                    self.set_status("Directory · Enter save · Esc cancel");
                }
            }
            KeyCode::Enter => {
                if let Some(picker) = self.harness_picker.take() {
                    if let Some(provider) = picker.providers.get(picker.index).copied() {
                        let cwd = picker.cwd.clone();
                        self.spawn_harness(provider, &cwd);
                    }
                }
            }
            _ => {}
        }
    }

    /// Route a key while the hand-back question is open.
    ///
    /// Enter means yes, because handing back is the safe answer: a harness left
    /// under a user who has walked away is one the orchestrator can never use,
    /// and that failure is silent.
    pub(super) fn handle_handback_key(&mut self, code: KeyCode) {
        let Some(prompt) = self.handback_prompt.as_ref() else {
            return;
        };
        let session = prompt.session.clone();
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.handback_prompt = None;
                self.hand_back_session(&session);
                self.release_harness();
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.handback_prompt = None;
                self.release_harness();
                self.set_status(
                    "Released · you still hold this harness (/handoff to give it back)",
                );
            }
            // Esc is "I did not mean to leave", so it puts the operator back
            // where they were rather than picking one of the answers for them.
            KeyCode::Esc => {
                self.handback_prompt = None;
                self.set_status("Still typing into the harness");
            }
            _ => {}
        }
    }
}
