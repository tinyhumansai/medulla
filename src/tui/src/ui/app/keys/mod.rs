//! Keyboard handling for [`App`]: the [`App::on_key`] dispatcher that routes key
//! events by active overlay, global control chords, and per-tab bindings. It
//! leans on helpers defined in [`super::input`], [`super::commands`], and
//! [`super::state`].
//!
//! Tasks, Routing, Memory, and Settings host subpages with bindings of their own,
//! so their handling lives in focused sibling modules rather than inline here.
//!
//! The Agents tab carries the composer, which settles every binding conflict on
//! that tab: printable keys always type, so no message loses a character to a
//! shortcut. The arrows belong to the caret and the history, exactly as they did
//! on the old Chat tab; lane selection moves to `Alt`+`↑`/`↓`, transcript
//! scrolling to `PageUp`/`PageDown`, and task steering to `Alt`+`X`/`Alt`+`A`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::types::{App, Cmd, TABS};
use crate::ui::command::CopyScope;
use crate::ui::composer::{
    delete_before, edit_prompt, insert_at, move_caret_row, Draft, PromptAction,
};

mod agents;
mod memory;
mod routing;
mod settings;
mod tasks;
#[cfg(feature = "workflows")]
mod workflows;

use agents::AgentsKey;
use memory::MemoryKey;
use routing::RoutingKey;
use settings::SettingsKey;
use tasks::TasksKey;
#[cfg(feature = "workflows")]
use workflows::WorkflowsKey;

impl App {
    /// Handle a key press for the current overlay/tab, producing any follow-up
    /// command the event loop must run.
    pub(super) fn on_key(&mut self, k: KeyEvent) -> Option<Cmd> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let alt = k.modifiers.contains(KeyModifiers::ALT);

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

        // The inline prompt (Workers add/edit, Agents answer) owns input while open.
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
                    self.runtime.abort();
                    self.set_status("Abort requested");
                    return None;
                }
                KeyCode::Char('n') => {
                    self.new_thread();
                    return None;
                }
                // Walk the open threads. The bare arrows belong to the composer,
                // so switching between conversations takes the control chord.
                KeyCode::Up | KeyCode::Down if tab == "Agents" => {
                    let idx = self.active_thread_idx();
                    let next = if matches!(k.code, KeyCode::Up) {
                        idx.checked_sub(1)
                    } else {
                        Some(idx + 1)
                    };
                    if let Some(thread) = next.and_then(|n| self.snapshot.threads.get(n)) {
                        let id = thread.id.clone();
                        self.runtime.set_active_thread(id);
                        self.chat_scroll = 0;
                        self.agent_scroll = 0;
                        self.refresh_snapshot();
                    }
                    return None;
                }
                _ => {}
            }
        }

        // Settings owns a nav plus seven subpages; it gets first refusal on
        // every key so its subpage bindings are not shadowed by the global ones.
        if tab == "Settings" {
            if let SettingsKey::Handled(cmd) = self.on_settings_key(k.code) {
                return *cmd;
            }
        }
        if tab == "Routing" {
            if let RoutingKey::Handled(cmd) = self.on_routing_key(k.code) {
                return cmd;
            }
        }
        if tab == "Tasks" {
            if let TasksKey::Handled(cmd) = self.on_tasks_key(k.code) {
                return cmd;
            }
        }
        if tab == "Memory" {
            if let MemoryKey::Handled(cmd) = self.on_memory_key(k.code) {
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

        // The command peek owns navigation and completion while it is open. It
        // opens on a typed `/` and closes as soon as one is chosen, so these
        // overrides last for exactly as long as there is a choice on screen.
        if self.command_suggestions().is_some() {
            match k.code {
                KeyCode::Up | KeyCode::Down => {
                    self.move_command_index(matches!(k.code, KeyCode::Up));
                    return None;
                }
                KeyCode::Tab => {
                    self.complete_command();
                    return None;
                }
                // Enter runs the peeked command rather than the half-typed
                // prefix: picking `/quit` from the list and pressing Enter must
                // quit, not report `/qu` as unknown.
                KeyCode::Enter if !shift && !alt => {
                    self.complete_command();
                    let value = self.draft.text.clone();
                    return self.execute(value);
                }
                KeyCode::Esc => {
                    self.draft = Draft::new();
                    self.command_index = 0;
                    return None;
                }
                _ => {}
            }
        }

        // The rail claims the bare arrows while it holds focus. Placed after the
        // global chords and the command peek so neither is shadowed, and before
        // the composer bindings, which otherwise take every arrow and character.
        if tab == "Agents" {
            if let AgentsKey::Handled(cmd) = self.on_agents_rail_key(k) {
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
            KeyCode::PageUp if tab == "Agents" => {
                let step = self.visible_count().saturating_sub(1).max(1);
                self.scroll_transcript(true, step);
            }
            KeyCode::PageDown if tab == "Agents" => {
                let step = self.visible_count().saturating_sub(1).max(1);
                self.scroll_transcript(false, step);
            }
            KeyCode::Enter if tab == "Agents" && (shift || alt) => {
                self.draft = insert_at(&self.draft.text, self.draft.cursor, "\n");
            }
            // The popup owns Esc and its own scrolling while it is up.
            KeyCode::Esc if tab == "Agents" && self.template_modal => {
                self.template_modal = false;
                self.template_scroll = 0;
            }
            KeyCode::PageUp | KeyCode::PageDown if tab == "Agents" && self.template_modal => {
                let step = 5;
                self.template_scroll = if matches!(k.code, KeyCode::PageUp) {
                    self.template_scroll.saturating_sub(step)
                } else {
                    self.template_scroll.saturating_add(step)
                };
            }
            // A template row has no transcript to show, so Enter opens its
            // declaration instead of submitting an empty instruction.
            KeyCode::Enter
                if tab == "Agents"
                    && self.draft.text.is_empty()
                    && self
                        .selected_fleet_node()
                        .is_some_and(|node| node.key.starts_with("template:")) =>
            {
                self.template_modal = !self.template_modal;
                self.template_scroll = 0;
            }
            // Enter submits to whatever the cursor is on: an instruction to the
            // orchestrator, or an answer to the selected agent's open question.
            // Steering an agent from the lane the question appeared on is the
            // whole point of folding the composer in here.
            KeyCode::Enter if tab == "Agents" => {
                let value = self.draft.text.clone();
                if let Some(cmd) = self.answer_from_composer(&value) {
                    return cmd;
                }
                return self.execute(value);
            }
            KeyCode::Backspace | KeyCode::Delete if tab == "Agents" => {
                self.draft = delete_before(&self.draft.text, self.draft.cursor);
                self.command_index = 0;
            }
            // Esc clears a draft first — that is the destructive-looking action
            // and must stay one keypress away from a half-typed message. With
            // nothing to clear it steps out to the rail, which is how a keyboard
            // reaches the lane list on a terminal that cannot send Alt+Arrow.
            KeyCode::Esc if tab == "Agents" => {
                if self.draft.text.is_empty() {
                    self.focus_agents_rail();
                } else {
                    self.draft = Draft::new();
                }
            }
            KeyCode::Left if tab == "Agents" => {
                self.draft.cursor = self.draft.cursor.saturating_sub(1);
            }
            KeyCode::Right if tab == "Agents" => {
                self.draft.cursor = (self.draft.cursor + 1).min(self.draft.text.chars().count());
            }
            // Lane selection: `Alt` because the bare arrows now belong to the
            // composer, and a lane list you cannot reach from the keyboard would
            // make the merged surface worse than the two it replaced.
            KeyCode::Up | KeyCode::Down if tab == "Agents" && alt => {
                self.agent_scroll = 0;
                self.chat_scroll = 0;
                self.move_agent_index(matches!(k.code, KeyCode::Up));
            }
            // Agents steering: cancel the selected running task, answer a pending
            // question through the modal prompt. Enter answers it inline too.
            KeyCode::Char('X') | KeyCode::Char('x') if tab == "Agents" && alt => {
                self.cancel_selected_task();
                return None;
            }
            KeyCode::Char('A') | KeyCode::Char('a') if tab == "Agents" && alt => {
                self.answer_selected_task();
                return None;
            }
            KeyCode::Up => {
                if tab == "Agents" {
                    if let Some(moved) = move_caret_row(&self.draft.text, self.draft.cursor, -1) {
                        self.draft.cursor = moved;
                    } else {
                        self.recall_older();
                    }
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if tab == "Agents" {
                    if let Some(moved) = move_caret_row(&self.draft.text, self.draft.cursor, 1) {
                        self.draft.cursor = moved;
                    } else {
                        self.recall_newer();
                    }
                } else {
                    self.selected += 1;
                }
            }
            KeyCode::Char(c) if tab == "Agents" && !ctrl && !alt => {
                self.draft = insert_at(&self.draft.text, self.draft.cursor, &c.to_string());
                // Narrowing the list invalidates where the cursor was pointing.
                self.command_index = 0;
            }
            _ => {}
        }
        None
    }
}
