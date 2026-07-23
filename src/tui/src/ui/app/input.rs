//! Event routing and pointer input for [`App`]: the top-level [`App::on_event`]
//! dispatch, mouse scroll/click handling, tab hit-testing, and the small
//! navigation helpers (agent-row movement, prompt-history recall, mouse toggle,
//! thread fork). Keyboard handling proper lives in [`super::keys`].

use crossterm::event::{Event, KeyEventKind, MouseButton, MouseEventKind};

use crate::ui::agents::{agent_row_model, AgentRow};
use crate::ui::composer::Draft;

use super::types::{App, Cmd};

impl App {
    /// Route a terminal event to the key or mouse handler, producing any command
    /// the event loop must run.
    pub fn on_event(&mut self, ev: Event) -> Option<Cmd> {
        match ev {
            Event::Key(k) if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.on_key(k)
            }
            Event::Mouse(m) => self.on_mouse(m),
            _ => None,
        }
    }

    /// Handle scroll and left-click mouse events for the active tab.
    pub(super) fn on_mouse(&mut self, m: crossterm::event::MouseEvent) -> Option<Cmd> {
        if self.resume_picker.is_some() {
            return None; // modal swallows mouse
        }
        let tab = self.tab();
        match m.kind {
            // Trace and Context are Settings subpages, not tabs, so they are
            // matched on the subpage rather than on `tab` — which is always
            // "Settings" for both, and used to make these arms unreachable.
            MouseEventKind::ScrollUp => match (tab, self.settings_subpage()) {
                ("Chat", _) => self.chat_scroll += 3,
                ("Agents", _) => self.agent_scroll += 3,
                ("Repo", _) => self.repo.diff_scroll = self.repo.diff_scroll.saturating_sub(3),
                ("Memory", _) => self.memory_index = self.memory_index.saturating_sub(1),
                ("Settings", "Trace") => self.selected = self.selected.saturating_sub(3),
                ("Settings", "Context") => {
                    self.context_index = self.context_index.saturating_sub(1)
                }
                _ => {}
            },
            MouseEventKind::ScrollDown => match (tab, self.settings_subpage()) {
                ("Chat", _) => self.chat_scroll = self.chat_scroll.saturating_sub(3),
                ("Agents", _) => self.agent_scroll = self.agent_scroll.saturating_sub(3),
                ("Repo", _) => self.repo.diff_scroll = self.repo.diff_scroll.saturating_add(3),
                ("Memory", _) => {
                    let max = self.memory_entry_count().saturating_sub(1);
                    self.memory_index = (self.memory_index + 1).min(max);
                }
                ("Settings", "Trace") => self.selected += 3,
                ("Settings", "Context") => {
                    let max = self.contexts.len().saturating_sub(1);
                    self.context_index = (self.context_index + 1).min(max);
                }
                _ => {}
            },
            MouseEventKind::Down(MouseButton::Left) => return self.handle_click(m.column, m.row),
            _ => {}
        }
        None
    }

    /// Resolve a left click at `(x, y)` to a tab switch or a row selection in the
    /// Agents, Context, or Chat panes.
    pub(super) fn handle_click(&mut self, x: u16, y: u16) -> Option<Cmd> {
        // Tab bar.
        if y == self.hit_tabs_row {
            for (i, (start, end)) in self.hit_tabs.clone().into_iter().enumerate() {
                if x >= start && x <= end {
                    self.tab_index = i;
                    self.selected = 0;
                    return self.tab_enter_cmd();
                }
            }
            return None;
        }
        let tab = self.tab();
        if tab == "Agents" {
            if let Some((rect, window_start)) = self.hit_agents {
                if rect.contains((x, y).into()) {
                    let rel = (y - rect.y) as usize;
                    let rows = self.agent_rows();
                    let idx = window_start + rel;
                    if let Some(row) = rows.get(idx) {
                        if row.selectable() {
                            self.agent_scroll = 0;
                            self.agent_index = idx;
                        }
                    }
                }
            }
        } else if tab == "Settings" && self.settings_subpage() == "Context" {
            // Context is a Settings subpage, not a tab — matching on `tab` here
            // made this branch unreachable, so clicking a chunk did nothing.
            if let Some(rect) = self.hit_context {
                if rect.contains((x, y).into()) {
                    let rel = (y - rect.y) as usize;
                    if rel < self.contexts.len() {
                        self.context_index = rel;
                    }
                }
            }
        } else if tab == "Chat" {
            if let Some((rect, window_start)) = self.hit_threads {
                if rect.contains((x, y).into()) {
                    let rel = (y - rect.y) as usize;
                    let idx = window_start + rel;
                    if let Some(t) = self.snapshot.threads.get(idx) {
                        let id = t.id.clone();
                        self.runtime.set_active_thread(id);
                        self.chat_scroll = 0;
                        self.refresh_snapshot();
                    }
                }
            }
        }
        None
    }

    /// The current Agents-list rows (lanes flattened with a hidden-row cap).
    pub(super) fn agent_rows(&self) -> Vec<AgentRow> {
        agent_row_model(&self.lanes(), 8)
    }

    /// The number of body rows a list pane can show for the current terminal
    /// height.
    pub(super) fn visible_count(&self) -> usize {
        (self.area.height as usize).saturating_sub(13).max(5)
    }

    /// Move the Agents-list cursor to the next/previous selectable row.
    pub(super) fn move_agent_index(&mut self, up: bool) {
        let rows = self.agent_rows();
        if rows.is_empty() {
            return;
        }
        let clamped = self.agent_index.min(rows.len() - 1);
        let step: i64 = if up { -1 } else { 1 };
        let mut next = clamped as i64 + step;
        while next >= 0 && (next as usize) < rows.len() && !rows[next as usize].selectable() {
            next += step;
        }
        self.agent_index = if next < 0 || next as usize >= rows.len() {
            clamped
        } else {
            next as usize
        };
    }

    /// Recall an older prompt from history into the composer.
    pub(super) fn recall_older(&mut self) {
        let next = (self.history.len() as i64 - 1).min(self.history_index + 1);
        if next >= 0 {
            self.history_index = next;
            let recalled = self
                .history
                .get(self.history.len() - 1 - next as usize)
                .cloned()
                .unwrap_or_default();
            self.draft = Draft {
                cursor: recalled.chars().count(),
                text: recalled,
            };
        }
    }

    /// Recall a newer prompt from history (or clear back to an empty draft).
    pub(super) fn recall_newer(&mut self) {
        if self.history_index >= 0 {
            let next = self.history_index - 1;
            self.history_index = next;
            let recalled = if next >= 0 {
                self.history
                    .get(self.history.len() - 1 - next as usize)
                    .cloned()
                    .unwrap_or_default()
            } else {
                String::new()
            };
            self.draft = Draft {
                cursor: recalled.chars().count(),
                text: recalled,
            };
        }
    }

    /// Toggle mouse capture and note the new mode in the status line.
    pub(super) fn toggle_mouse(&mut self) {
        self.mouse_capture = !self.mouse_capture;
        self.set_status(if self.mouse_capture {
            "Mouse captured — click tabs/lanes to navigate, wheel scrolls (Shift/Option-drag to copy)"
        } else {
            "Mouse released — native click-drag selection & copy restored"
        });
    }

    /// Fork the active thread (optionally named), reset chat scroll, and refresh.
    pub(super) fn fork_thread(&mut self, name: Option<String>) {
        let label = name.clone().unwrap_or_else(|| "new thread".into());
        self.runtime.fork(name);
        self.chat_scroll = 0;
        self.refresh_snapshot();
        self.set_status(format!("Forked → {label} (inherits history; fresh fleet)"));
    }
}
