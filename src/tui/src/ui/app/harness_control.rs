//! Starting harnesses the operator owns, and moving control between them and
//! the orchestrator.
//!
//! Two features that turn out to be one. "Unmanaged" is not a kind of harness —
//! it is a harness the operator holds, and dispatch skips anything the operator
//! holds. So spawning one, taking one over, and handing one back are three
//! spellings of the same state change, and they live together here.
//!
//! The state changes here are synchronous. Opening a PTY and setting a flag are
//! both immediate, so the operator gets an answer on the status line in the same
//! keystroke — which is the whole point of a control handover being *explicit*.
//!
//! Telling the *orchestrator* is not immediate, so that part is queued as a
//! [`Cmd`](super::types::Cmd) and travels off-thread. Control flips locally
//! first and the brief follows: a handback gated on a socket round-trip would
//! fail whenever the uplink is down, which is exactly when an operator most
//! wants to let go of a harness.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use medulla::protocol::HarnessProvider;

use crate::ui::composer::Draft;
use crate::ui::harness_pane::HarnessChoice;
use crate::worker::pty::HarnessControl;

use super::types::{
    tab_pos, App, Cmd, HandbackPolicy, HandbackPrompt, HarnessPicker, HarnessPickerStep,
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
                self.spawn_harness(HarnessChoice::native(provider), &cwd);
            }
            None => {
                let choices = harnesses.choices();
                if choices.is_empty() {
                    self.set_status("No harness CLIs found on this device");
                    return;
                }
                self.harness_picker = Some(HarnessPicker {
                    choices,
                    index: 0,
                    step: HarnessPickerStep::Harness,
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
    pub(crate) fn open_harness_picker(&mut self) {
        self.start_harness_command(None, None);
    }

    /// Start a harness the operator owns and move the cursor onto it.
    ///
    /// Selecting the new row matters more than it sounds: a harness that
    /// appears somewhere below the fold, with the pane still showing whatever
    /// was selected before, reads as "nothing happened".
    pub(super) fn spawn_harness(&mut self, choice: HarnessChoice, cwd: &str) {
        let Some(harnesses) = self.harnesses.clone() else {
            self.set_status("This device is not hosting, so it has no harnesses to start");
            return;
        };
        let skip = self.harness_skip_permissions;
        let workspace = harnesses.resolve_workspace(cwd);
        match harnesses.open_unmanaged(&choice, &workspace, skip) {
            Ok(id) => {
                self.tab_index = tab_pos("Agents");
                self.select_harness_row(&id);
                let mut status = format!(
                    "Started {} · unmanaged, the orchestrator will not use it",
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
        if let Some(cwd) = harnesses.sessions.row(&session).map(|row| row.cwd) {
            self.pending_cmds.push_back(Cmd::HoldHarness {
                workspace: cwd,
                reason: None,
            });
        }
        self.set_status("You have this harness · the orchestrator will not dispatch into it");
    }

    /// Give the selected harness back to the orchestrator, with an optional note.
    pub(crate) fn hand_harness_back(&mut self, note: Option<String>) {
        let Some((harnesses, session)) = self.handoff_target() else {
            return;
        };
        if harnesses.control(&session) == Some(HarnessControl::Orchestrator) {
            self.set_status("The orchestrator already has this harness");
            return;
        }
        self.hand_back_session(&session, note);
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
            Some(HarnessControl::User) => self.hand_back_session(&session, None),
            Some(HarnessControl::Orchestrator) => self.take_harness_control(),
            None => self.set_status("That harness is gone"),
        }
    }

    /// The harness `/handoff` means, without depending on a render having run.
    ///
    /// [`selected_harness`](Self::selected_harness) reads `harness_pane_session`,
    /// which is written inside the Agents pane's draw and cleared at the top of
    /// every frame. So `/handoff` typed from any other tab reported "no harness
    /// on this row" while the operator was demonstrably holding one — and with a
    /// note argument that is worse, because they have just typed a sentence that
    /// is then thrown away.
    ///
    /// In order: the attached session (unambiguous — the keyboard is in it), the
    /// harness the last frame resolved, then the single running harness the
    /// operator holds. Ambiguity is reported, never guessed: handing back the
    /// wrong harness puts an agent into a workspace somebody is still using.
    fn handoff_target(&mut self) -> Option<(crate::ui::harness_pane::LocalHarnesses, String)> {
        let Some(harnesses) = self.harnesses.clone() else {
            self.set_status("This device is not hosting, so it has no harnesses");
            return None;
        };
        if let Some(session) = self.harness_focus.attached_to() {
            return Some((harnesses, session.to_string()));
        }
        if let Some(session) = self.harness_pane_session.clone() {
            return Some((harnesses, session));
        }
        let held: Vec<String> = harnesses
            .sessions
            .rows()
            .into_iter()
            .filter(|row| row.control == HarnessControl::User && row.state.is_running())
            .map(|row| row.id)
            .collect();
        match held.len() {
            0 => {
                self.set_status("You are not holding any harness");
                None
            }
            1 => Some((harnesses, held[0].clone())),
            n => {
                self.set_status(format!(
                    "You hold {n} harnesses — select one in Agents and press Ctrl-G"
                ));
                None
            }
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
                self.hand_back_session(session, None);
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
                    note: Draft::default(),
                    editing_note: false,
                });
                false
            }
        }
    }

    /// The note typed into the handback prompt, when it is not blank.
    fn handback_note(&self) -> Option<String> {
        self.handback_prompt
            .as_ref()
            .map(|prompt| prompt.note.text.trim().to_string())
            .filter(|note| !note.is_empty())
    }

    /// Apply one keystroke to the handback prompt's note.
    ///
    /// A single line, so it uses the shared draft primitives directly rather
    /// than the full composer: there is nothing here to submit, wrap, or
    /// history-scroll.
    fn edit_handback_note(&mut self, code: KeyCode) {
        let Some(prompt) = self.handback_prompt.as_mut() else {
            return;
        };
        let draft = &mut prompt.note;
        match code {
            KeyCode::Char(c) => {
                *draft = crate::ui::composer::insert_at(&draft.text, draft.cursor, &c.to_string());
            }
            KeyCode::Backspace => {
                *draft = crate::ui::composer::delete_before(&draft.text, draft.cursor);
            }
            KeyCode::Left => draft.cursor = draft.cursor.saturating_sub(1),
            KeyCode::Right => draft.cursor = (draft.cursor + 1).min(draft.text.chars().count()),
            _ => {}
        }
    }

    /// Take a bracketed-paste payload into the hand-back note.
    ///
    /// The question itself owns the keyboard and holds no field — `y`, `n` and
    /// `E` are answers, not text — so a paste made while it is up belongs to
    /// neither the harness behind it nor the composer, and is dropped. After `E`
    /// the note *is* a text input, and pasting what you were doing into the
    /// brief the orchestrator receives is exactly what the note is for.
    ///
    /// Flattened to one line and inserted at the caret, matching
    /// [`edit_handback_note`](Self::edit_handback_note): the note is drawn as a
    /// single row, and `Enter` there hands the harness back rather than breaking
    /// the line.
    pub(super) fn paste_into_handback_note(&mut self, text: &str) {
        let Some(prompt) = self.handback_prompt.as_mut() else {
            return;
        };
        if !prompt.editing_note {
            return;
        }
        let draft = &mut prompt.note;
        *draft = crate::ui::composer::insert_at(
            &draft.text,
            draft.cursor,
            &crate::ui::composer::flatten_paste(text),
        );
    }

    /// Hand `session` back and queue its brief. Every handback path ends here.
    ///
    /// The order matters. The transcript is read while the harness is still
    /// ours; control flips next, so the operator gets an answer on the same
    /// keystroke; the brief is queued last and travels asynchronously.
    ///
    /// That ordering means the orchestrator can dispatch into the harness before
    /// it has read the brief, and that is the right trade. The brief is
    /// *context*, not permission — gating the flip on a socket round-trip would
    /// make handing a harness back fail whenever the uplink is down, which is
    /// exactly when an operator most wants to let go of one.
    pub(super) fn hand_back_session(&mut self, session: &str, note: Option<String>) {
        let Some(harnesses) = self.harnesses.clone() else {
            return;
        };
        // Read the row first: a session that has already gone is not handed
        // back, and flipping control on a corpse would advertise a harness that
        // does not exist.
        let Some(row) = harnesses.sessions.row(session) else {
            self.set_status("That harness is gone");
            return;
        };
        let lines = harnesses
            .sessions
            .tail_lines(session, medulla::hub::handoff::TRANSCRIPT_LINES);

        harnesses.set_control(session, HarnessControl::Orchestrator);
        self.harness_took_control = false;

        let brief = medulla::hub::handoff::normalize(
            medulla::hub::HarnessHandoff {
                // Per handback *event*, not per session: a second handback of the
                // same harness is new work, and reusing the id would have the
                // orchestrator ignore it as something it already picked up.
                id: format!("{}-{}", row.id, medulla::clock::now_millis()),
                at: medulla::clock::now_millis(),
                session_id: row.id.clone(),
                harness_session_id: row.session_id.clone(),
                provider: row.provider.as_str().to_string(),
                workspace_path: row.cwd.clone(),
                // Filled off-thread: reading them shells out to git.
                branch: None,
                project: None,
                note,
                transcript: String::new(),
                transcript_truncated: false,
            },
            &lines,
        );
        self.pending_cmds
            .push_back(Cmd::HandOffHarness(Box::new(brief)));
        self.set_status("Handed back · sending the orchestrator your brief");
    }
}

impl App {
    /// Route a key while the "start a harness" picker is open.
    ///
    /// The first step chooses a registered harness. The second step owns text
    /// input directly so filtering and filesystem completion update as the
    /// operator types.
    pub(super) fn handle_harness_picker_key(&mut self, event: KeyEvent) {
        let code = event.code;
        let step = self
            .harness_picker
            .as_ref()
            .map(|picker| picker.step)
            .unwrap_or(HarnessPickerStep::Harness);
        if step == HarnessPickerStep::Workspace {
            self.handle_harness_workspace_key(event);
            return;
        }
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
                    picker.index = (picker.index + 1).min(picker.choices.len().saturating_sub(1));
                }
            }
            KeyCode::Char('e') if is_text_input(event.modifiers) => {
                self.open_harness_workspace_step(true);
            }
            KeyCode::Enter => {
                self.open_harness_workspace_step(false);
            }
            _ => {}
        }
    }

    /// Route a key while choosing and completing the workspace directory.
    fn handle_harness_workspace_key(&mut self, event: KeyEvent) {
        match event.code {
            KeyCode::Esc | KeyCode::BackTab => {
                if let Some(picker) = &mut self.harness_picker {
                    picker.step = HarnessPickerStep::Harness;
                }
                self.set_status("Pick a harness · Enter workspace · Esc cancel");
            }
            // Moving the cursor is the operator choosing a completion over
            // whatever they entered, however few rows there are to move across.
            KeyCode::Up => {
                if let Some(picker) = &mut self.harness_picker {
                    picker.workspace_index = picker.workspace_index.saturating_sub(1);
                    picker.workspace_picked = !picker.workspace_choices.is_empty();
                }
            }
            KeyCode::Down => {
                if let Some(picker) = &mut self.harness_picker {
                    picker.workspace_index = (picker.workspace_index + 1)
                        .min(picker.workspace_choices.len().saturating_sub(1));
                    picker.workspace_picked = !picker.workspace_choices.is_empty();
                }
            }
            KeyCode::Tab => self.complete_harness_workspace(),
            KeyCode::Backspace => {
                if let Some(picker) = &mut self.harness_picker {
                    picker.workspace_query.pop();
                    picker.workspace_index = 0;
                    picker.workspace_picked = false;
                }
                self.refresh_harness_workspace_choices();
            }
            KeyCode::Char(character) if is_text_input(event.modifiers) => {
                if let Some(picker) = &mut self.harness_picker {
                    picker.workspace_query.push(character);
                    picker.workspace_index = 0;
                    picker.workspace_picked = false;
                }
                self.refresh_harness_workspace_choices();
            }
            KeyCode::Enter => {
                let Some(workspace) = self.selected_harness_workspace() else {
                    self.set_status("Choose an existing directory");
                    return;
                };
                let choice = self
                    .harness_picker
                    .as_ref()
                    .and_then(|picker| picker.choices.get(picker.index).cloned());
                let Some(choice) = choice else {
                    self.set_status("Choose a harness first");
                    return;
                };
                self.harness_picker = None;
                self.spawn_harness(choice, &workspace);
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
        // While the note is being typed, every key is text. `y` and `n` have to
        // keep meaning yes and no, so an operator whose note begins "no, the
        // migration…" must not have the first letter answer for them.
        if prompt.editing_note {
            match code {
                KeyCode::Enter => {
                    let note = self.handback_note();
                    self.handback_prompt = None;
                    self.hand_back_session(&session, note);
                    self.release_harness();
                }
                // Back to the question, keeping what was typed: an operator who
                // pressed Escape meant "stop typing", not "discard my sentence".
                KeyCode::Esc => {
                    if let Some(prompt) = self.handback_prompt.as_mut() {
                        prompt.editing_note = false;
                    }
                }
                _ => self.edit_handback_note(code),
            }
            return;
        }
        match code {
            KeyCode::Char('e') | KeyCode::Char('E') => {
                if let Some(prompt) = self.handback_prompt.as_mut() {
                    prompt.editing_note = true;
                }
            }
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                let note = self.handback_note();
                self.handback_prompt = None;
                self.hand_back_session(&session, note);
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

/// Only printable text belongs in the workspace query; modifier chords must
/// never be mistaken for their underlying character.
pub(super) fn is_text_input(modifiers: KeyModifiers) -> bool {
    modifiers == KeyModifiers::NONE
        || modifiers == KeyModifiers::SHIFT
        || modifiers == (KeyModifiers::CONTROL | KeyModifiers::ALT)
}
