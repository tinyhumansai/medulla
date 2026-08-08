//! Closing a local harness and reconciling its UI ownership state.
//!
//! This is isolated from the handoff lifecycle because ending a process settles
//! that lifecycle immediately: a stopped harness cannot be returned to the
//! orchestrator or remain attached to the keyboard.

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
    /// Killing settles the handover question rather than raising it. There is no
    /// session left to hand back, so a take recorded against it is dropped
    /// instead of being turned into a hand-back prompt about a corpse — and the
    /// keyboard comes back to the chrome, because the pane it was in has stopped
    /// listening.
    pub(crate) fn close_session(&mut self, session: &str) {
        let Some(harnesses) = self.local_sessions.clone() else {
            self.set_status("This device is not hosting, so it has no sessions");
            return;
        };
        let was_taken = self.sessions_taken.contains_key(session);
        let release_workspace =
            was_taken && !self.workspace_has_another_taken_session(&harnesses, session);
        if !harnesses.sessions.close(session) {
            self.set_status("That session is gone");
            return;
        }
        // `close` retains the row long enough for `hand_back_session` to build
        // its brief. A taken session also held its workspace in the backend,
        // but that hold is shared by every other locally-held session in the
        // same workspace. Only the last such session may release it.
        if release_workspace {
            self.hand_back_session(session, None);
        }
        if self.harness_focus.is_attached_to(session) {
            self.release_session();
        }
        self.sessions_taken.remove(session);
        self.set_status("Closed the harness");
    }

    /// Whether another running, operator-held session keeps `session`'s
    /// workspace reserved from orchestrator dispatch.
    ///
    /// Holds are workspace-scoped in the backend, while [`sessions_taken`]
    /// records individual sessions. Looking up the live rows prevents an old
    /// record for an already-exited session from indefinitely preserving a hold.
    fn workspace_has_another_taken_session(
        &self,
        harnesses: &crate::ui::harness_pane::LocalSessions,
        session: &str,
    ) -> bool {
        let Some(workspace) = harnesses.sessions.row(session).map(|row| row.cwd) else {
            return false;
        };
        self.sessions_taken.keys().any(|other_session| {
            other_session != session
                && harnesses
                    .sessions
                    .row(other_session)
                    .is_some_and(|row| row.state.is_running() && row.cwd == workspace)
        })
    }
}
