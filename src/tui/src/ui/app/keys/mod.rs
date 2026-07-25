//! Keyboard handling for [`App`]: the [`App::on_key`] dispatcher that routes key
//! events by active overlay, global control chords, and per-tab bindings. It
//! leans on helpers defined in [`super::input`], [`super::commands`], and
//! [`super::state`].
//!
//! Tasks, Routing, Memory, and Settings host subpages with bindings of their own,
//! so their handling lives in focused sibling modules rather than inline here.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::types::{tab_pos, App, Cmd, TABS};
use crate::ui::command::CopyScope;
use crate::ui::composer::{
    delete_before, edit_prompt, insert_at, move_caret_row, Draft, PromptAction,
};

mod memory;
mod routing;
mod settings;
mod tasks;

use memory::MemoryKey;
use routing::RoutingKey;
use settings::SettingsKey;
use tasks::TasksKey;

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
                    self.runtime.new_session();
                    self.refresh_snapshot();
                    self.set_status("Started a fresh conversation session");
                    return None;
                }
                KeyCode::Char('f') => {
                    self.fork_thread(None);
                    if tab != "Chat" {
                        self.tab_index = tab_pos("Chat");
                    }
                    return None;
                }
                KeyCode::Up | KeyCode::Down if tab == "Chat" => {
                    let idx = self.active_thread_idx();
                    let next = if matches!(k.code, KeyCode::Up) {
                        idx.checked_sub(1)
                    } else {
                        Some(idx + 1)
                    };
                    if let Some(n) = next {
                        if let Some(t) = self.snapshot.threads.get(n) {
                            let id = t.id.clone();
                            self.runtime.set_active_thread(id);
                            self.chat_scroll = 0;
                            self.refresh_snapshot();
                        }
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
            KeyCode::PageUp if tab == "Chat" => {
                self.chat_scroll += self.visible_count().saturating_sub(1).max(1);
            }
            KeyCode::PageDown if tab == "Chat" => {
                let step = self.visible_count().saturating_sub(1).max(1);
                self.chat_scroll = self.chat_scroll.saturating_sub(step);
            }
            KeyCode::Enter if tab == "Chat" && (shift || alt) => {
                self.draft = insert_at(&self.draft.text, self.draft.cursor, "\n");
            }
            KeyCode::Enter if tab == "Chat" => {
                let value = self.draft.text.clone();
                return self.execute(value);
            }
            KeyCode::Backspace | KeyCode::Delete if tab == "Chat" => {
                self.draft = delete_before(&self.draft.text, self.draft.cursor);
            }
            KeyCode::Esc if tab == "Chat" => {
                self.draft = Draft::new();
            }
            KeyCode::Left if tab == "Chat" => {
                self.draft.cursor = self.draft.cursor.saturating_sub(1);
            }
            KeyCode::Right if tab == "Chat" => {
                self.draft.cursor = (self.draft.cursor + 1).min(self.draft.text.chars().count());
            }
            KeyCode::Up | KeyCode::Down if tab == "Agents" && self.draft.text.is_empty() => {
                self.agent_scroll = 0;
                self.move_agent_index(matches!(k.code, KeyCode::Up));
            }
            KeyCode::Char('j') if tab == "Agents" && self.draft.text.is_empty() => {
                self.agent_scroll += 1;
            }
            KeyCode::Char('k') if tab == "Agents" && self.draft.text.is_empty() => {
                self.agent_scroll = self.agent_scroll.saturating_sub(1);
            }
            // Agents steering: cancel the selected running task, answer a pending question.
            KeyCode::Char('X') if tab == "Agents" => {
                self.cancel_selected_task();
                return None;
            }
            KeyCode::Char('A') if tab == "Agents" => {
                self.answer_selected_task();
                return None;
            }
            KeyCode::Up => {
                if tab == "Chat" {
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
                if tab == "Chat" {
                    if let Some(moved) = move_caret_row(&self.draft.text, self.draft.cursor, 1) {
                        self.draft.cursor = moved;
                    } else {
                        self.recall_newer();
                    }
                } else {
                    self.selected += 1;
                }
            }
            KeyCode::Char(c) if tab == "Chat" && !ctrl && !alt => {
                self.draft = insert_at(&self.draft.text, self.draft.cursor, &c.to_string());
            }
            _ => {}
        }
        None
    }
}
