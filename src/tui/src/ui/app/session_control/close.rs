//! Closing a local harness and reconciling its UI focus state.

use super::super::types::App;

impl App {
    /// Ask before closing the harness the pane is showing.
    ///
    /// A question rather than a kill, for the same reason the task-kill chord
    /// asks: the child is usually mid-turn, and what it loses is not recoverable
    /// by pressing the key again. Refusing with a reason beats doing nothing —
    /// an operator who pressed `k` and saw nothing cannot tell "wrong row" from
    /// "broken".
    pub(crate) fn close_pane_session_prompt(&mut self) {
        let Some(session) = self.pane_session.clone() else {
            self.set_status("No session on this row — select one to close it");
            return;
        };
        let running = self
            .local_sessions
            .as_ref()
            .is_some_and(|harnesses| harnesses.is_running(&session));
        if !running {
            self.set_status("That session has already exited");
            return;
        }
        self.arm_harness_close(session);
    }

    /// Close a harness: kill the child, and tidy up what held it.
    ///
    /// Killing also releases the attachment, returning the keyboard to the
    /// chrome because the pane it was in has stopped listening.
    pub(crate) fn close_session(&mut self, session: &str) {
        let Some(harnesses) = self.local_sessions.clone() else {
            self.set_status("This device is not hosting, so it has no sessions");
            return;
        };
        if !harnesses.sessions.close(session) {
            self.set_status("That session is gone");
            return;
        }
        if self.harness_focus.is_attached_to(session) {
            self.release_session();
        }
        self.set_status("Closed the harness");
    }
}
