//! Keyboard navigation for the session Git changes view.
//!
//! Three cursors share the tab: the file rail (`↑↓`), the diff line the review
//! cursor rests on (`j`/`k`), and the hunk the cursor jumps between (`[`/`]`).
//! All of them are reachable without a mouse, which is what makes the comment
//! bindings addressable at all.

use crossterm::event::KeyCode;

use super::super::types::App;

impl App {
    /// Handle Changes-specific navigation before generic list bindings.
    pub(super) fn on_changes_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Up => self.changes.select_file(-1),
            KeyCode::Down => self.changes.select_file(1),
            KeyCode::Char('k') => self.changes.move_cursor(-1),
            KeyCode::Char('j') => self.changes.move_cursor(1),
            KeyCode::Char('[') => {
                if !self.changes.jump_hunk(false) {
                    self.set_status("Already at the first hunk");
                }
            }
            KeyCode::Char(']') => {
                if !self.changes.jump_hunk(true) {
                    self.set_status("Already at the last hunk");
                }
            }
            // The diff pane scrolls to follow the review cursor, so paging moves
            // the cursor rather than the viewport; otherwise the next redraw
            // would snap the view straight back to where the cursor was left.
            KeyCode::PageUp => self.changes.move_cursor(-10),
            KeyCode::PageDown => self.changes.move_cursor(10),
            KeyCode::Char('r') => self.refresh_changes(),
            KeyCode::Char('c') => self.comment_on_change(),
            KeyCode::Char('C') => self.comment_on_change_file(),
            KeyCode::Char('e') => self.edit_change_comment(),
            _ => return false,
        }
        true
    }
}
