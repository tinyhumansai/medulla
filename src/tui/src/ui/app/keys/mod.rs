//! Keyboard handling for [`App`]: the [`App::on_key`] dispatcher that routes key
//! events by active overlay, global control chords, and per-tab bindings. It
//! leans on helpers defined in [`super::input`], [`super::commands`], and
//! [`super::state`].
//!
//! The Settings tab now hosts seven subpages with bindings of their own, so its
//! handling lives in [`settings`] rather than inline here.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ui::command::CopyScope;
use crate::ui::composer::{delete_before, insert_at, move_caret_row, Draft};
use medulla::runtime::WorkerOp;

use super::types::{tab_pos, App, Cmd, Prompt, PromptKind, TABS};

mod settings;

use settings::SettingsKey;

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
            match k.code {
                KeyCode::Char('c') if ctrl => self.should_quit = true,
                KeyCode::Esc => {
                    self.prompt = None;
                    self.set_status("Cancelled");
                }
                KeyCode::Enter => return self.submit_prompt(),
                KeyCode::Backspace | KeyCode::Delete => {
                    if let Some(p) = &mut self.prompt {
                        p.draft = delete_before(&p.draft.text, p.draft.cursor);
                    }
                }
                KeyCode::Left => {
                    if let Some(p) = &mut self.prompt {
                        p.draft.cursor = p.draft.cursor.saturating_sub(1);
                    }
                }
                KeyCode::Right => {
                    if let Some(p) = &mut self.prompt {
                        p.draft.cursor = (p.draft.cursor + 1).min(p.draft.text.chars().count());
                    }
                }
                KeyCode::Char(c) if !ctrl && !alt => {
                    if let Some(p) = &mut self.prompt {
                        p.draft = insert_at(&p.draft.text, p.draft.cursor, &c.to_string());
                    }
                }
                _ => {}
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
                return cmd;
            }
        }

        match k.code {
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
            // Workers fleet ops.
            KeyCode::Up if tab == "Workers" => {
                self.worker_index = self.worker_index.saturating_sub(1);
            }
            KeyCode::Down if tab == "Workers" => {
                let max = self.runtime.workers().len().saturating_sub(1);
                self.worker_index = (self.worker_index + 1).min(max);
            }
            KeyCode::Char('a') if tab == "Workers" => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::WorkerAdd,
                    title: "Add worker — address or @handle, optional label".into(),
                    draft: Draft::new(),
                });
                self.set_status("Add worker · Enter save · Esc cancel");
            }
            KeyCode::Char('s') | KeyCode::Enter if tab == "Workers" => {
                if let Some(w) = self.selected_worker() {
                    self.set_status(format!(
                        "Selecting {}",
                        w.label.as_deref().unwrap_or(&w.address)
                    ));
                    return Some(Cmd::WorkerOp(WorkerOp::Select { id: w.id }));
                }
            }
            KeyCode::Char('d') | KeyCode::Char('x') if tab == "Workers" => {
                if let Some(w) = self.selected_worker() {
                    self.set_status(format!(
                        "Removing {}",
                        w.label.as_deref().unwrap_or(&w.address)
                    ));
                    return Some(Cmd::WorkerOp(WorkerOp::Remove { id: w.id }));
                }
            }
            KeyCode::Char('e') if tab == "Workers" => {
                if let Some(w) = self.selected_worker() {
                    let mut draft = Draft::new();
                    if let Some(l) = &w.label {
                        draft = insert_at("", 0, l);
                    }
                    self.prompt = Some(Prompt {
                        kind: PromptKind::WorkerEditLabel(w.id.clone()),
                        title: format!("Edit label — {}", w.address),
                        draft,
                    });
                    self.set_status("Edit label · Enter save · Esc cancel");
                }
            }
            // Memory browse.
            KeyCode::Up if tab == "Memory" => {
                self.memory_index = self.memory_index.saturating_sub(1);
            }
            KeyCode::Down if tab == "Memory" => {
                let max = self.memory_entry_count().saturating_sub(1);
                self.memory_index = (self.memory_index + 1).min(max);
            }
            KeyCode::Char('j') if tab == "Memory" && self.draft.text.is_empty() => {
                let max = self.memory_entry_count().saturating_sub(1);
                self.memory_index = (self.memory_index + 1).min(max);
            }
            KeyCode::Char('k') if tab == "Memory" && self.draft.text.is_empty() => {
                self.memory_index = self.memory_index.saturating_sub(1);
            }
            // Memory maintenance. Ingest calls a paid provider, so both modes
            // refuse to start a second run while one is in flight.
            KeyCode::Char('r') if tab == "Memory" && self.draft.text.is_empty() => {
                self.set_status("Memory · refreshing…");
                return Some(Cmd::LoadMemory);
            }
            KeyCode::Char('b') | KeyCode::Char('i')
                if tab == "Memory" && self.draft.text.is_empty() =>
            {
                let backfill = matches!(k.code, KeyCode::Char('b'));
                return self.start_memory_ingest(backfill);
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
