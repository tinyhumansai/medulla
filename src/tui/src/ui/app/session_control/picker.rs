//! Starting local harness sessions and presenting the harness-type picker.
//!
//! This module owns both picker state transitions and workspace-key routing.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use medulla::protocol::HarnessProvider;

use crate::ui::harness_pane::HarnessChoice;

use super::super::types::{tab_pos, App, Prompt, PromptKind, SessionPicker, SessionPickerStep};
use crate::ui::composer::Draft;

impl App {
    /// Open the "start a session" picker, or spawn directly when the command
    /// already named a harness type.
    ///
    /// `/session` with no harness type opens the picker rather than guessing:
    /// starting the wrong CLI in the operator's workspace is not something they
    /// find out about until it has already done something.
    pub(crate) fn start_session_command(&mut self, provider: Option<&str>, path: Option<&str>) {
        let Some(harnesses) = self.local_sessions.clone() else {
            self.set_status("This device is not hosting, so it has no sessions to start");
            return;
        };
        match provider.and_then(HarnessProvider::from_wire) {
            Some(provider) => {
                let cwd = path.unwrap_or("").to_string();
                self.spawn_session(HarnessChoice::native(provider), &cwd);
            }
            None => {
                let choices = harnesses.choices();
                if choices.is_empty() {
                    self.set_status("No harness CLIs found on this device");
                    return;
                }
                self.session_picker = Some(SessionPicker {
                    choices,
                    index: 0,
                    step: SessionPickerStep::Harness,
                    cwd: path
                        .map(str::to_string)
                        .unwrap_or_else(|| harnesses.workspace.clone()),
                    workspace_query: String::new(),
                    workspace_choices: Vec::new(),
                    workspace_index: 0,
                    workspace_picked: false,
                });
            }
        }
    }

    /// Open the picker from the keyboard shortcut.
    pub(crate) fn open_session_picker(&mut self) {
        self.start_session_command(None, None);
    }

    /// Start a session the operator owns and move the cursor onto it.
    ///
    /// Always unmanaged, and not as a default the operator can override: a
    /// session started by hand is one somebody intends to type into, and the
    /// orchestrator starts its own managed without being asked. Spawning one
    /// into dispatch would mean the very next thing the operator does — press
    /// Enter on the row they just created — is a request to take it back off
    /// the orchestrator it was handed to a keystroke earlier.
    ///
    /// Selecting the new row matters more than it sounds: a session that
    /// appears somewhere below the fold, with the pane still showing whatever
    /// was selected before, reads as "nothing happened".
    pub(crate) fn spawn_session(&mut self, choice: HarnessChoice, cwd: &str) {
        let Some(harnesses) = self.local_sessions.clone() else {
            self.set_status("This device is not hosting, so it has no sessions to start");
            return;
        };
        let skip = self.harness_skip_permissions;
        let workspace = harnesses.resolve_workspace(cwd);
        match harnesses.open_unmanaged(&choice, &workspace, skip) {
            Ok(id) => {
                self.tab_index = tab_pos("Sessions");
                self.select_session_row(&id);
                let mut status = format!(
                    "Started {} in {workspace} · local session",
                    choice.display_name()
                );
                if let Err(error) = self.remember_harness_workspace(&workspace) {
                    status.push_str(&format!(" · {error}"));
                }
                self.set_status(status);
            }
            // Surfaced, never swallowed: a spawn that fails silently leaves the
            // operator waiting for a pane that is never coming.
            Err(err) => {
                self.set_status(format!("Could not start {}: {err}", choice.display_name()))
            }
        }
    }

    /// Route a key while the harness picker is open.
    pub(crate) fn handle_session_picker_key(&mut self, event: KeyEvent) {
        let step = self
            .session_picker
            .as_ref()
            .map(|picker| picker.step)
            .unwrap_or(SessionPickerStep::Harness);
        if step == SessionPickerStep::Workspace {
            self.handle_harness_workspace_key(event);
            return;
        }
        match event.code {
            KeyCode::Esc => {
                self.session_picker = None;
                self.set_status("Cancelled");
            }
            KeyCode::Up => {
                if let Some(picker) = &mut self.session_picker {
                    picker.index = picker.index.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let Some(picker) = &mut self.session_picker {
                    picker.index = (picker.index + 1).min(picker.choices.len().saturating_sub(1));
                }
            }
            KeyCode::Char('e') if is_text_input(event.modifiers) => {
                self.open_harness_workspace_step(true)
            }
            KeyCode::Enter => self.open_harness_workspace_step(false),
            _ => {}
        }
    }

    /// Route a key while completing a workspace directory.
    fn handle_harness_workspace_key(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::Esc | KeyCode::BackTab => {
                if let Some(picker) = &mut self.session_picker {
                    picker.step = SessionPickerStep::Harness;
                }
                self.set_status("Pick a harness type · Enter workspace · Esc cancel");
            }
            KeyCode::Up => {
                if let Some(picker) = &mut self.session_picker {
                    picker.workspace_index = picker.workspace_index.saturating_sub(1);
                    picker.workspace_picked = !picker.workspace_choices.is_empty();
                }
            }
            KeyCode::Down => {
                if let Some(picker) = &mut self.session_picker {
                    picker.workspace_index = (picker.workspace_index + 1)
                        .min(picker.workspace_choices.len().saturating_sub(1));
                    picker.workspace_picked = !picker.workspace_choices.is_empty();
                }
            }
            KeyCode::Tab => self.complete_harness_workspace(),
            KeyCode::Char('F') if event.modifiers == KeyModifiers::SHIFT => {
                let Some(workspace) = self.selected_picker_workspace() else {
                    self.set_status("Choose an existing directory before saving a favorite");
                    return;
                };
                self.prompt = Some(Prompt {
                    kind: PromptKind::FavoriteWorkspaceAdd(workspace.clone()),
                    title: format!("Save favorite for {workspace}"),
                    draft: Draft::new(),
                });
                self.set_status("Favorite name · Enter save · Esc cancel");
            }
            KeyCode::Backspace => {
                if let Some(picker) = &mut self.session_picker {
                    picker.workspace_query.pop();
                    picker.workspace_index = 0;
                    picker.workspace_picked = false;
                }
                self.refresh_harness_workspace_choices();
            }
            KeyCode::Char(character) if is_text_input(event.modifiers) => {
                if let Some(picker) = &mut self.session_picker {
                    picker.workspace_query.push(character);
                    picker.workspace_index = 0;
                    picker.workspace_picked = false;
                }
                self.refresh_harness_workspace_choices();
            }
            KeyCode::Enter => {
                let Some(workspace) = self.selected_picker_workspace() else {
                    self.set_status("Choose an existing directory");
                    return;
                };
                let Some(choice) = self
                    .session_picker
                    .as_ref()
                    .and_then(|picker| picker.choices.get(picker.index).cloned())
                else {
                    self.set_status("Choose a harness type first");
                    return;
                };
                self.session_picker = None;
                self.spawn_session(choice, &workspace);
            }
            _ => {}
        }
    }
}

/// Return whether modifiers represent ordinary printable text input.
pub(in crate::ui::app) fn is_text_input(modifiers: KeyModifiers) -> bool {
    modifiers == KeyModifiers::NONE
        || modifiers == KeyModifiers::SHIFT
        || modifiers == (KeyModifiers::CONTROL | KeyModifiers::ALT)
}
