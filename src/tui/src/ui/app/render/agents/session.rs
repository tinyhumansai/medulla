//! Which session the Agents cursor is on, and what that answer arms.
//!
//! One question, asked once per draw, with three consequences: the pane and the
//! rail both need the id so they draw the same session, the keyboard needs it so
//! `Ctrl-]` attaches to the session the operator is looking at, and a row this
//! device does *not* host has to be told apart from a row that names no session
//! at all — one is watchable and the other is nothing, and both leave the local
//! session empty.
//!
//! It lives beside the layout rather than in it because the layout only consumes
//! the answer: a session paints its own composer, so resolving it is what
//! decides how many rows the split has to give away.

use super::super::super::rail::RailRow;
use super::super::super::types::App;
use super::types::Selection;

impl App {
    /// Resolve the session `selection` points at and record it for the next key
    /// press.
    ///
    /// Fills in [`Selection::session`] and, from it, the pane and rail pointers
    /// and the remote-session classification. Called while `selection` is still
    /// being built, so everything downstream — layout, drawing, the keyboard —
    /// reads one answer.
    pub(super) fn resolve_selected_session(&mut self, selection: &mut Selection) {
        selection.session = self.local_session(selection);
        // Focus follows the pane, not the other way round. If the cursor moved
        // off the attached session — or that session ended — the keyboard comes
        // back to the chrome, because keys landing in a harness the operator is
        // no longer looking at is the worst failure this feature can have.
        if let Some(attached) = self.harness_focus.attached_to() {
            if selection.session.as_deref() != Some(attached) {
                self.release_session();
            }
        }
        self.pane_session = selection.session.clone();
        self.rail_session = selection.session.clone();
        // A session row this device is not running: watchable, but not takeable
        // (§E7). Recorded here because this is the only place that can tell the
        // difference — one row down the cursor, both cases are a `None` session.
        self.pane_remote_session = match (&selection.session, selection.rows.get(selection.active))
        {
            (None, Some(RailRow::Session(row))) => Some(
                row.agent_id
                    .clone()
                    .unwrap_or_else(|| "another host".to_string()),
            ),
            _ => None,
        };
    }
}
