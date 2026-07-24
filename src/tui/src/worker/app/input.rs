//! Terminal-event and pointer routing for the worker TUI.
//!
//! The worker captures the mouse for clickable setup choices, tabs, and list
//! rows. Ctrl-O releases that capture when the operator needs the terminal's
//! native drag-selection and copy behavior.

use crossterm::event::{Event, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};

use super::types::{Screen, WorkerApp, WorkerCmd, TAB_CONTACTS, TAB_REQUESTS, TAB_SESSIONS};

impl WorkerApp {
    /// Route a terminal event to the worker's keyboard or pointer handler.
    pub fn on_event(&mut self, event: Event) -> Option<WorkerCmd> {
        match event {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                self.on_key(key)
            }
            Event::Mouse(mouse) => self.on_mouse(mouse),
            _ => None,
        }
    }

    /// Handle click and wheel events using hit boxes recorded by the last draw.
    fn on_mouse(&mut self, mouse: MouseEvent) -> Option<WorkerCmd> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.handle_click(mouse.column, mouse.row),
            MouseEventKind::ScrollUp => {
                self.handle_scroll(true);
                None
            }
            MouseEventKind::ScrollDown => {
                self.handle_scroll(false);
                None
            }
            _ => None,
        }
    }

    /// Resolve a click against setup choices, tabs, or the active list.
    fn handle_click(&mut self, x: u16, y: u16) -> Option<WorkerCmd> {
        if self.screen == Screen::Setup {
            let area = self.hit_setup?;
            if area.contains((x, y).into()) {
                return self.answer_setup((y - area.y) as usize);
            }
            return None;
        }

        let (tab_row, ranges) = &self.hit_tabs;
        if y == *tab_row {
            if let Some(index) = ranges
                .iter()
                .position(|(start, end)| x >= *start && x <= *end)
            {
                self.set_tab(index);
            }
            return None;
        }

        let (area, first) = self.hit_rows?;
        if !area.contains((x, y).into()) {
            return None;
        }
        let index = first + (y - area.y) as usize;
        match self.tab {
            TAB_SESSIONS if !self.is_headless() && index < self.session_rows().len() => {
                self.session_index = index;
            }
            TAB_CONTACTS if index < self.accepted_contacts().len() => {
                self.contact_index = index;
            }
            TAB_REQUESTS if index < self.pending_requests().len() => {
                self.request_index = index;
            }
            _ => {}
        }
        None
    }

    /// Apply wheel movement to the setup cursor, log, or active list.
    fn handle_scroll(&mut self, up: bool) {
        if self.screen == Screen::Setup {
            let last = match self.setup_step {
                super::types::SetupStep::Mode => super::types::EXECUTION_MODES.len(),
                super::types::SetupStep::Harness => self.providers.len(),
            }
            .saturating_sub(1);
            self.setup_index = if up {
                self.setup_index.saturating_sub(1)
            } else {
                (self.setup_index + 1).min(last)
            };
        } else if self.tab == TAB_SESSIONS && self.is_headless() {
            self.log_scroll = if up {
                self.log_scroll.saturating_add(3)
            } else {
                self.log_scroll.saturating_sub(3)
            };
        } else {
            self.move_cursor(up);
        }
    }

    /// Toggle between app pointer input and native terminal text selection.
    pub(super) fn toggle_mouse(&mut self) {
        self.mouse_capture = !self.mouse_capture;
        self.set_status(if self.mouse_capture {
            "Mouse captured — click to navigate; Ctrl-O releases it for text selection"
        } else {
            "Mouse released — native drag-select and copy restored; Ctrl-O recaptures it"
        });
    }
}
