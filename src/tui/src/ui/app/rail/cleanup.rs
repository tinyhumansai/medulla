//! Retiring finished work from the Sessions rail.
//!
//! The rail is a picture of what is happening on this device, and everything on
//! it eventually stops happening: a harness exits, a workflow run settles. Kept
//! forever, those rows accumulate until the live ones are a minority of the
//! list — and the operator has to read a session's glyph to tell which of a
//! dozen entries is still doing something.
//!
//! So a session whose child is gone leaves the rail, and with it the screen and
//! scrollback it was holding. Three things pin one anyway, because each needs
//! to remain visible to an operator:
//!
//! - the session the keyboard is attached to — the operator is reading that
//!   screen right now, exit code and all, and it is often *why* they attached;
//! - a session with a workflow run still executing. A detached run outlives its
//!   parent harness by design ([`medulla::control_socket::runs`]), and its rows
//!   are drawn under the session that started it, so sweeping the session would
//!   take the run's own progress off screen with it.
//! - a failed child, whose exit cue explains why the dispatch stopped and lets
//!   the operator inspect or dismiss its retained record.
//!
//! Settled runs are dropped from under a session the same way, by
//! [`run_rows_under`](super::run_rows_under). The durable account of every run
//! is the workflow store's, which the Workflows page reads — nothing is lost
//! here that is not written down somewhere an operator can go back to.

use crate::worker::pty::SessionRow;

use super::super::types::App;

impl App {
    /// Whether a session whose child has exited still belongs on the rail.
    ///
    /// See the module doc for the three cases. Everything else is history the
    /// rail no longer shows.
    pub(in crate::ui::app) fn keeps_finished_session(&self, row: &SessionRow) -> bool {
        if self.harness_focus.is_attached_to(&row.id) {
            return true;
        }
        // A failed child has lifecycle information the operator needs to see.
        // Keeping it also prevents the frame sweep from deleting its cue before
        // the rail has rendered even once.
        if crate::worker::pty::lifecycle_cue(row, medulla::clock::now_millis())
            .is_some_and(|cue| cue.kind.is_failure())
        {
            return true;
        }
        row.mcp_grant_session
            .as_deref()
            .is_some_and(|grant| self.harness_runs.any_active_for_session(grant))
    }

    /// Forget the finished sessions the rail has stopped listing.
    ///
    /// Hiding a row is not enough on its own: the manager would go on holding
    /// the session's record and its scrollback for the life of the process,
    /// reachable by nothing — a leak in exactly the shape the old "forget it
    /// yourself" gesture existed to prevent. Called from the frame tick, so a
    /// session leaves within a frame of the rule that keeps it lapsing.
    ///
    /// [`PtyManager::forget`](crate::worker::pty::PtyManager::forget) refuses
    /// while a child is alive, so a race with an exit that has not been noticed
    /// yet costs nothing but a retry on the next tick.
    pub fn sweep_finished_sessions(&mut self) {
        let Some(harnesses) = self.local_sessions.as_ref() else {
            return;
        };
        let stale: Vec<String> = harnesses
            .sessions
            .rows()
            .into_iter()
            .filter(|row| !row.state.is_running() && !self.keeps_finished_session(row))
            .map(|row| row.id)
            .collect();
        for id in &stale {
            harnesses.sessions.forget(id);
        }
    }
}
