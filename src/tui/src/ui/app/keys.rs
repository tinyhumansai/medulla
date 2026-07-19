//! Keyboard handling for [`App`]: the single large [`App::on_key`] dispatcher
//! that routes key events by active overlay, global control chords, and per-tab
//! bindings. It leans on helpers defined in [`super::input`], [`super::commands`],
//! and [`super::state`].

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ui::command::CopyScope;
use crate::ui::composer::{delete_before, insert_at, move_caret_row, Draft};
use crate::ui::theme::THEME_ROLES;
use medulla::client::FeedbackType;
use medulla::runtime::WorkerOp;

use super::types::{
    tab_pos, App, Cmd, Prompt, PromptKind, SETTINGS_SUBPAGES, SP_APPEARANCE, SP_CONFIG, SP_USAGE,
    TABS,
};

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
            // Settings tab: subpage navigation + the Appearance theme editor.
            KeyCode::Char(d @ '1'..='4') if tab == "Settings" => {
                return self.set_settings_subpage(d as usize - '1' as usize);
            }
            KeyCode::Char('r') if tab == "Settings" && self.settings_index == SP_USAGE => {
                self.set_status("Usage · refreshing…");
                return Some(Cmd::LoadUsage);
            }
            KeyCode::Char('c') if tab == "Settings" && self.settings_index == SP_USAGE => {
                self.settings_index = SP_CONFIG;
            }
            KeyCode::Up | KeyCode::Down if tab == "Settings" => {
                let up = matches!(k.code, KeyCode::Up);
                self.settings_index = if up {
                    self.settings_index.saturating_sub(1)
                } else {
                    (self.settings_index + 1).min(SETTINGS_SUBPAGES.len() - 1)
                };
                return self.tab_enter_cmd();
            }
            KeyCode::Char('j') | KeyCode::Char('k')
                if tab == "Settings" && self.settings_index == SP_APPEARANCE =>
            {
                let up = matches!(k.code, KeyCode::Char('k'));
                self.appearance_index = if up {
                    self.appearance_index.saturating_sub(1)
                } else {
                    (self.appearance_index + 1).min(THEME_ROLES.len() - 1)
                };
            }
            KeyCode::Char('j') | KeyCode::Char('k') if tab == "Settings" => {
                let up = matches!(k.code, KeyCode::Char('k'));
                self.settings_index = if up {
                    self.settings_index.saturating_sub(1)
                } else {
                    (self.settings_index + 1).min(SETTINGS_SUBPAGES.len() - 1)
                };
                return self.tab_enter_cmd();
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Enter
                if tab == "Settings" && self.settings_index == SP_APPEARANCE =>
            {
                let forward = !matches!(k.code, KeyCode::Left);
                self.cycle_appearance_role(forward);
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
            // Feedback board: browse, vote, comment, submit.
            KeyCode::Up if tab == "Feedback" => {
                return self.move_feedback_index(true);
            }
            KeyCode::Down if tab == "Feedback" => {
                return self.move_feedback_index(false);
            }
            KeyCode::Char('k') if tab == "Feedback" => {
                return self.move_feedback_index(true);
            }
            KeyCode::Char('j') if tab == "Feedback" => {
                return self.move_feedback_index(false);
            }
            KeyCode::Char('u') if tab == "Feedback" => {
                return self.vote_selected_feedback(1);
            }
            KeyCode::Char('d') if tab == "Feedback" => {
                return self.vote_selected_feedback(-1);
            }
            KeyCode::Char('c') if tab == "Feedback" => {
                self.open_feedback_comment();
                return None;
            }
            KeyCode::Char('n') if tab == "Feedback" => {
                self.open_feedback_submit(FeedbackType::Feature);
                return None;
            }
            KeyCode::Char('b') if tab == "Feedback" => {
                self.open_feedback_submit(FeedbackType::Bug);
                return None;
            }
            KeyCode::Char('s') if tab == "Feedback" => {
                return self.cycle_feedback_sort();
            }
            KeyCode::Char('f') if tab == "Feedback" => {
                return self.cycle_feedback_filter();
            }
            KeyCode::Char('r') | KeyCode::Enter if tab == "Feedback" => {
                self.set_status("Feedback · refreshing…");
                return self.reload_feedback();
            }
            KeyCode::Char('j') if tab == "Memory" && self.draft.text.is_empty() => {
                let max = self.memory_entry_count().saturating_sub(1);
                self.memory_index = (self.memory_index + 1).min(max);
            }
            KeyCode::Char('k') if tab == "Memory" && self.draft.text.is_empty() => {
                self.memory_index = self.memory_index.saturating_sub(1);
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
            KeyCode::Char('j') if tab == "Context" && self.draft.text.is_empty() => {
                let max = self.contexts.len().saturating_sub(1);
                self.context_index = (self.context_index + 1).min(max);
            }
            KeyCode::Char('k') if tab == "Context" && self.draft.text.is_empty() => {
                self.context_index = self.context_index.saturating_sub(1);
            }
            KeyCode::Char(c) if tab == "Chat" && !ctrl && !alt => {
                self.draft = insert_at(&self.draft.text, self.draft.cursor, &c.to_string());
            }
            _ => {}
        }
        None
    }
}
