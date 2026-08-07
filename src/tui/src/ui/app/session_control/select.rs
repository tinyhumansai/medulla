//! Putting the rail cursor on a session that was just started or focused.
//!
//! Selecting the new row matters more than it sounds: a session that appears
//! somewhere below the fold, with the pane still showing whatever was selected
//! before, reads as "nothing happened".

use super::super::types::App;

impl App {
    /// Put the rail cursor on the row for `session_id`, if it has one.
    pub(in crate::ui::app) fn select_session_row(&mut self, session_id: &str) {
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        if let Some(index) = rows
            .iter()
            .position(|row| row.session_id() == Some(session_id))
        {
            self.set_rail_cursor_in(&rows, &lanes, index);
        }
    }
}
