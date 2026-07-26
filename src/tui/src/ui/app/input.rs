//! Event routing and pointer input for [`App`]: the top-level [`App::on_event`]
//! dispatch, mouse scroll/click handling, tab hit-testing, and the small
//! navigation helpers (rail movement, prompt-history recall, mouse toggle).
//! Keyboard handling proper lives in [`super::keys`].

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
                ("Agents", _) => self.scroll_transcript(true, 3),
                ("Memory", _) if self.memory_focused => {
                    self.memory_index = self.memory_index.saturating_sub(1)
                }
                ("Settings", "Trace") => self.selected = self.selected.saturating_sub(3),
                ("Settings", "Context") => {
                    self.context_index = self.context_index.saturating_sub(1)
                }
                _ => {}
            },
            MouseEventKind::ScrollDown => match (tab, self.settings_subpage()) {
                ("Agents", _) => self.scroll_transcript(false, 3),
                ("Memory", _) if self.memory_focused => {
                    let max = self.memory_page_entries().len().saturating_sub(1);
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
            // The rail stacks two hit boxes — threads above lanes — so both are
            // tried; an `else if` here would leave the strip unclickable.
            if let Some((rect, window_start)) = self.hit_threads {
                if rect.contains((x, y).into()) {
                    let rel = (y - rect.y) as usize;
                    let idx = window_start + rel;
                    if let Some(t) = self.snapshot.threads.get(idx) {
                        let id = t.id.clone();
                        self.runtime.set_active_thread(id);
                        self.chat_scroll = 0;
                        self.agent_scroll = 0;
                        self.refresh_snapshot();
                    }
                }
            }
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

    /// Move the Agents-rail cursor to the next/previous selectable row.
    ///
    /// The rail spans the lanes and the declared fleet, so this walks straight
    /// from the last agent into the first host rather than stopping short.
    pub(super) fn move_agent_index(&mut self, up: bool) {
        let rows = self.rail_rows();
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

    /// Open a new thread and focus the conversation.
    ///
    /// A thread is opened, not reset: several conversations can be in flight at
    /// once, and clearing the one you were in would throw away the transcript
    /// you were keeping. Nothing is inherited from the current thread — that is
    /// the whole difference from the fork this replaced.
    pub(super) fn new_thread(&mut self) {
        self.runtime.new_session();
        self.draft = crate::ui::composer::Draft::new();
        self.chat_scroll = 0;
        self.agent_scroll = 0;
        self.agent_index = 0;
        self.tab_index = super::types::tab_pos("Agents");
        self.refresh_snapshot();
        let name = self
            .snapshot
            .threads
            .get(self.active_thread_idx())
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "main".into());
        self.set_status(format!("Opened {name} · ^↑↓ switches threads"));
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
}
