//! Keyboard handling for [`App`]: the [`App::on_key`] dispatcher that routes key
//! events by active overlay, global control chords, and per-tab bindings. It
//! leans on helpers defined in [`super::input`], [`super::commands`], and
//! [`super::state`].
//!
//! Routing and Settings host subpages with bindings of their own,
//! so their handling lives in focused sibling modules rather than inline here.
//!
//! The Sessions tab used to carry the orchestrator's composer, and every binding
//! on it was arranged around that: printable keys always typed, the bare arrows
//! drove the caret, and rail selection was pushed onto `Alt`+`↑`/`↓`. The
//! composer went with the orchestrator, so the bare arrows walk the rail again;
//! `Alt`+`↑`/`↓` still does too, because that is what fingers learned. Transcript
//! scrolling stays on `PageUp`/`PageDown` and task steering on
//! `Alt`+`X`/`Alt`+`A`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::types::{App, Cmd, TABS};
use crate::ui::command::CopyScope;
use crate::ui::composer::{edit_prompt, PromptAction};

mod changes;
mod harness;
mod routing;
mod sessions;
mod settings;
mod tokenmaxxing;
#[cfg(feature = "workflows")]
mod workflows;

use routing::RoutingKey;
pub(in crate::ui::app) use sessions::SessionsKey;
use settings::SettingsKey;
use tokenmaxxing::TokenMaxxxingKey;
#[cfg(feature = "workflows")]
use workflows::WorkflowsKey;

impl App {
    /// Handle a key press for the current overlay/tab, producing any follow-up
    /// command the event loop must run.
    pub(super) fn on_key(&mut self, k: KeyEvent) -> Option<Cmd> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let alt = k.modifiers.contains(KeyModifiers::ALT);

        // Killing a harness can lose in-progress work. Once armed, the prompt
        // owns exactly one keypress: only a deliberate `y` proceeds.
        if let Some((worker, task_id)) = self.kill_armed.take() {
            if k.code == KeyCode::Char('y') && k.modifiers.is_empty() {
                self.set_status(format!("Killing the session for {task_id}…"));
                return Some(Cmd::KillTask { worker, task_id });
            }
            self.set_status("Session kill cancelled");
            return None;
        }

        // Closing a harness ends whatever turn it was in the middle of, so the
        // same one-keypress contract applies: `y` closes it, anything else is a
        // "no" — including the keys that would otherwise have done something,
        // which is the point of the question owning the keyboard.
        if let Some(session) = self.harness_close_armed.take() {
            if k.code == KeyCode::Char('y') && k.modifiers.is_empty() {
                self.close_session(&session);
            } else {
                self.set_status("Harness close cancelled");
            }
            return None;
        }

        // The hand-back question outranks even the attached harness, and has to:
        // it is asked *while still attached*, because releasing the keyboard
        // before it is answered would hide the pane the question is about. So
        // for the few keystrokes it is open, the chrome takes the keyboard back.
        if self.handback_prompt.is_some() {
            self.handle_handback_key(k.code);
            return None;
        }

        // An attached harness owns the keyboard outright — ahead of the
        // overlays and the quit chord both. Anything less is not a terminal:
        // the operator would be typing into Claude Code with a handful of keys
        // mysteriously reserved, and `Ctrl-C` (interrupt the harness) would quit
        // the orchestrator instead.
        if self.handle_harness_key(k) {
            return None;
        }

        // A picker may open the ordinary inline prompt to edit one of its
        // values. Route that prompt first while leaving the picker behind it,
        // ready to resume once the edit is submitted or cancelled.
        if self.prompt.is_some() {
            if ctrl && k.code == KeyCode::Char('c') {
                self.should_quit = true;
            } else {
                let action = edit_prompt(self.prompt.as_mut().expect("prompt is present"), k);
                match action {
                    PromptAction::Cancel => {
                        self.prompt = None;
                        self.set_status("Cancelled");
                    }
                    PromptAction::Submit => return self.submit_prompt(),
                    PromptAction::Editing => {}
                }
            }
            return None;
        }

        // The harness picker owns navigation while open.
        if self.session_picker.is_some() {
            if ctrl && k.code == KeyCode::Char('c') {
                self.should_quit = true;
            } else {
                self.handle_session_picker_key(k);
            }
            return None;
        }

        // Resume picker owns navigation while open.
        if self.resume_picker.is_some() {
            match k.code {
                KeyCode::Char('c') if ctrl => self.should_quit = true,
                KeyCode::Esc => self.resume_picker = None,
                KeyCode::Up => {
                    if let Some(p) = &mut self.resume_picker {
                        p.index = p.index.saturating_sub(1);
                    }
                }
                KeyCode::Down => {
                    if let Some(p) = &mut self.resume_picker {
                        p.index = (p.index + 1).min(p.chats.len().saturating_sub(1));
                    }
                }
                KeyCode::Enter => {
                    if let Some(p) = self.resume_picker.take() {
                        if let Some(chat) = p.chats.get(p.index) {
                            return Some(Cmd::Resume(chat.session_id.clone()));
                        }
                    }
                }
                _ => {}
            }
            return None;
        }

        // The prepared-decision overlay owns navigation while open.
        if self.decision_open {
            if ctrl && k.code == KeyCode::Char('c') {
                self.should_quit = true;
            } else {
                self.handle_decision_key(k.code);
            }
            return None;
        }

        let tab = self.tab();

        // Global control chords.
        if ctrl {
            match k.code {
                KeyCode::Char('c') => {
                    self.should_quit = true;
                    return None;
                }
                KeyCode::Char('o') => {
                    self.toggle_mouse();
                    return None;
                }
                KeyCode::Char('y') => {
                    self.copy_chat(CopyScope::All);
                    return None;
                }
                KeyCode::Char('x') => {
                    // On the Workflows tab with a turn in flight, ⌃X means the
                    // copilot: it is the thing the operator is watching, and
                    // aborting the chat runtime instead would stop something
                    // they are not looking at while the pane kept spinning.
                    #[cfg(feature = "workflows")]
                    if tab == "Workflows" && self.copilot_busy() {
                        return self.abort_copilot();
                    }
                    self.runtime.abort();
                    self.set_status("Abort requested");
                    return None;
                }
                KeyCode::Char('n') => {
                    self.new_thread();
                    return None;
                }
                // Open a session. `Ctrl-T` for terminal; `Ctrl-N` is already a
                // new thread, which is the thing it would otherwise be confused
                // with.
                //
                // One door, from every row: pick a harness type, pick a
                // directory, and the session is open. It used to branch on the
                // row under the cursor — an agent row opened a session of *that*
                // agent, with a name prompt — so the same chord asked two
                // different questions depending on where the cursor happened to
                // be, and the fast case was the one nobody could predict.
                KeyCode::Char('t') => {
                    self.open_session_picker();
                    return None;
                }
                // Grab or give: one chord for both directions, because the rail
                // row and the pane title both say which way it will go.
                KeyCode::Char('g') => {
                    self.toggle_session_control();
                    return None;
                }
                // Walk the open threads. The bare arrows belong to the composer,
                // so switching between conversations takes the control chord.
                KeyCode::Up | KeyCode::Down if tab == "Sessions" => {
                    let idx = self.active_thread_idx();
                    let next = if matches!(k.code, KeyCode::Up) {
                        idx.checked_sub(1)
                    } else {
                        Some(idx + 1)
                    };
                    if let Some(thread) = next.and_then(|n| self.snapshot.threads.get(n)) {
                        let id = thread.id.clone();
                        self.runtime.set_active_thread(id);
                        self.agent_scroll = 0;
                        self.refresh_snapshot();
                    }
                    return None;
                }
                _ => {}
            }
        }

        // The Feedback tab is the same board as Settings › Feedback, so it
        // reuses that page's bindings; anything it does not bind still falls
        // through to the global ones.
        if tab == "Feedback" {
            if let SettingsKey::Handled(cmd) = self.feedback_key(k.code) {
                return *cmd;
            }
        }

        // Settings owns a nav plus eight subpages; it gets first refusal on
        // every key so its subpage bindings are not shadowed by the global ones.
        if tab == "Settings" {
            if let SettingsKey::Handled(cmd) = self.on_settings_key(k.code) {
                return *cmd;
            }
        }
        if tab == "Hosts" {
            if let RoutingKey::Handled(cmd) = self.on_routing_key(k.code) {
                return cmd;
            }
        }
        if tab == "TokenMaxxxing" {
            if let TokenMaxxxingKey::Handled(cmd) = self.on_tokenmaxxing_key(k.code) {
                return cmd;
            }
        }
        // Workflows owns three panes, one of which is a composer, so it gets
        // first refusal on every key that is not a global chord — exactly as
        // Settings and Routing do for their subpages.
        #[cfg(feature = "workflows")]
        if tab == "Workflows" {
            if let WorkflowsKey::Handled(cmd) = self.on_workflows_key(k.code, shift, alt) {
                return cmd;
            }
        }

        // The rail claims the bare arrows while it holds focus. Placed after the
        // global chords and the command peek so neither is shadowed, and before
        // the composer bindings, which otherwise take every arrow and character.
        if tab == "Sessions" {
            if let SessionsKey::Handled(cmd) = self.on_sessions_rail_key(k) {
                return cmd;
            }
        }

        match k.code {
            KeyCode::Char('E') if tab == "Overview" => {
                self.open_decisions();
                return None;
            }
            KeyCode::Tab => {
                self.tab_index = (self.tab_index + 1) % TABS.len();
                self.selected = 0;
                return self.tab_enter_cmd();
            }
            KeyCode::BackTab => {
                self.tab_index = (self.tab_index + TABS.len() - 1) % TABS.len();
                self.selected = 0;
                return self.tab_enter_cmd();
            }
            #[cfg(feature = "workflows")]
            KeyCode::PageUp if tab == "Sessions" => {
                if self.on_workflow_run_row().is_some() {
                    self.wf.preview_scroll = self.wf.preview_scroll.saturating_sub(6);
                } else {
                    let step = self.visible_count().saturating_sub(1).max(1);
                    self.scroll_transcript(true, step);
                }
            }
            #[cfg(feature = "workflows")]
            KeyCode::PageDown if tab == "Sessions" => {
                if self.on_workflow_run_row().is_some() {
                    self.wf.preview_scroll = self.wf.preview_scroll.saturating_add(6);
                } else {
                    let step = self.visible_count().saturating_sub(1).max(1);
                    self.scroll_transcript(false, step);
                }
            }
            #[cfg(not(feature = "workflows"))]
            KeyCode::PageUp if tab == "Sessions" => {
                let step = self.visible_count().saturating_sub(1).max(1);
                self.scroll_transcript(true, step);
            }
            #[cfg(not(feature = "workflows"))]
            KeyCode::PageDown if tab == "Sessions" => {
                let step = self.visible_count().saturating_sub(1).max(1);
                self.scroll_transcript(false, step);
            }
            // Kept on `Alt` as well as the bare arrows: the chord was the only
            // way to reach the rail while the composer held the bare ones, and
            // muscle memory outlives the composer that caused it.
            KeyCode::Up | KeyCode::Down if tab == "Sessions" && alt => {
                self.agent_scroll = 0;
                self.move_rail_index(matches!(k.code, KeyCode::Up));
                // Arrowing onto a task watches it, exactly as clicking does.
                if let Some(cmd) = self.retarget_watch() {
                    return Some(cmd);
                }
            }
            // Sessions steering: cancel the selected running task, answer a pending
            // question through the modal prompt. Enter answers it inline too.
            KeyCode::Char('X') | KeyCode::Char('x') if tab == "Sessions" && alt => {
                self.cancel_selected_task();
                return None;
            }
            KeyCode::Char('A') | KeyCode::Char('a') if tab == "Sessions" && alt => {
                self.answer_selected_task();
                return None;
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                self.selected += 1;
            }
            _ => {}
        }
        None
    }
}
