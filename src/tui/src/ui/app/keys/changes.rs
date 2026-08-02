//! Keyboard navigation for the session Git changes view.

use crossterm::event::KeyCode;

use super::super::types::App;

impl App {
    /// Handle Changes-specific navigation before generic list bindings.
    pub(super) fn on_changes_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Up => {
                self.changes.selected = self.changes.selected.saturating_sub(1);
                self.changes.reload_patch();
            }
            KeyCode::Down => {
                self.changes.selected =
                    (self.changes.selected + 1).min(self.changes.files.len().saturating_sub(1));
                self.changes.reload_patch();
            }
            KeyCode::PageUp => self.changes.scroll = self.changes.scroll.saturating_sub(10),
            KeyCode::PageDown => {
                self.changes.scroll = (self.changes.scroll + 10).min(self.changes.max_scroll);
            }
            KeyCode::Char('r') => self.refresh_changes(),
            KeyCode::Char('c') => self.comment_on_change(),
            _ => return false,
        }
        true
    }
}
