//! Closing a harness, local or remote, and reconciling its UI focus state.

use super::super::remote_sessions::SessionRef;
use super::super::types::App;

impl App {
    /// Close the harness the pane is showing, or dismiss it once it has exited.
    ///
    /// Two gestures behind one key, split on whether the child is still alive.
    ///
    /// A running child is a question rather than a kill, for the same reason the
    /// task-kill chord asks: it is usually mid-turn, and what it loses is not
    /// recoverable by pressing the key again.
    ///
    /// An exited one is dismissed, unless a detached run it started is still
    /// executing — the rail sweeps finished sessions on its own, but
    /// deliberately pins a *failed* child so its exit cue stays readable, and
    /// pins any child whose MCP grant still has an active run under it, since
    /// that run's rows are drawn under this session — see
    /// [`keeps_finished_session`](App::keeps_finished_session) — and that pin
    /// has no expiry. Without this the row is permanent: the sweep will not
    /// take it and the kill refused it for not running, which left the
    /// operator holding a dead row and no gesture that removed it. There is
    /// nothing to confirm, because there is no work left to lose.
    ///
    /// Routed by [`SessionRef`] like every other session action: a remote id
    /// (`host/w_1`) has no orchestrator control to check — a remote session is
    /// always [`SessionControl::User`](crate::worker::pty::SessionControl::User),
    /// see [`session_row`](super::super::remote_sessions::session_row) — so only
    /// whether it is still running is worth asking about.
    pub(crate) fn close_pane_session_prompt(&mut self) {
        let Some(session) = self.pane_session.clone() else {
            self.set_status("No session on this row — select one to close it");
            return;
        };
        let running = self
            .session_row(&session)
            .is_some_and(|row| matches!(row.state, crate::worker::pty::PtyState::Running));
        // Before the control check, not after: that refusal speaks only of
        // sessions that are still running, and applying it to a settled task
        // would pin the exited row the same way the old refusal did.
        if !running {
            self.dismiss_exited_session(&session);
            return;
        }
        if let SessionRef::Local(id) = SessionRef::parse(&session) {
            if self
                .local_sessions
                .as_ref()
                .and_then(|harnesses| harnesses.control(id))
                == Some(crate::worker::pty::SessionControl::Orchestrator)
            {
                self.set_status("Task sessions are view-only while they are running");
                return;
            }
        }
        self.arm_harness_close(session);
    }

    /// Drop an exited session's row, releasing the screen it still holds.
    ///
    /// Only the local manager owns a record this can remove.
    /// [`remote_session_rows`](App::remote_session_rows) is rebuilt from what
    /// each host reports, so forgetting a remote row here would restore it on
    /// the next update.
    ///
    /// That leaves a remote exited row with no dismissal, and the status says
    /// so rather than implying the host will deal with it: nothing on a
    /// headless host retires one. `sweep_finished_sessions` runs from the
    /// interactive frame loop only, and
    /// [`current_rows`](crate::remote::serve) publishes `PtyManager::rows()`
    /// verbatim — so a remote failure row is as permanent as a local one used
    /// to be. Closing that gap needs a host-side forget that can see the run
    /// registry, since the guard below has no counterpart there; tracked
    /// separately rather than bolted on here.
    fn dismiss_exited_session(&mut self, session: &str) {
        match SessionRef::parse(session) {
            SessionRef::Local(id) => {
                let Some(harnesses) = self.local_sessions.clone() else {
                    self.set_status("This device is not hosting, so it has no sessions");
                    return;
                };
                // The same protection `keeps_finished_session` gives the frame
                // sweep: `PtyManager::forget` revokes the row's MCP grant
                // unconditionally, and a detached workflow run outlives its
                // parent harness by design. Dismissing here first would pull
                // that grant out from under a run still reporting through it,
                // so this is refused exactly like a still-running child is.
                let has_active_run = self
                    .session_row(session)
                    .and_then(|row| row.mcp_grant_session)
                    .is_some_and(|grant| self.harness_runs.any_active_for_session(&grant));
                if has_active_run {
                    self.set_status(
                        "This session started a run that is still executing — \
                         it stays until the run settles",
                    );
                    return;
                }
                // Released first: the pane is about to stop having a session to
                // show, and an attachment outliving its row is what leaves the
                // keyboard typing into nothing.
                if self.harness_focus.is_attached_to(session) {
                    self.release_session();
                }
                if !harnesses.sessions.forget(id) {
                    self.set_status("That session is gone");
                    return;
                }
                self.set_status("Dismissed the exited session");
            }
            SessionRef::Remote { .. } => {
                self.set_status(
                    "That session has already exited — a remote row cannot be dismissed from here",
                );
            }
        }
    }

    /// Close a harness: kill the child, and tidy up what held it.
    ///
    /// Killing also releases the attachment, returning the keyboard to the
    /// chrome because the pane it was in has stopped listening. For a remote
    /// session this sends [`RemoteRequest::Close`](crate::remote::client::RemoteRequest::Close)
    /// to that host's connection; the daemon closing the pty is what actually
    /// ends the child, so [`release_session`](Self::release_session) runs
    /// immediately rather than waiting for the host to confirm — the same as the
    /// local path, which does not wait for the child to finish exiting either.
    pub(crate) fn close_session(&mut self, session: &str) {
        match SessionRef::parse(session) {
            SessionRef::Local(id) => {
                let Some(harnesses) = self.local_sessions.clone() else {
                    self.set_status("This device is not hosting, so it has no sessions");
                    return;
                };
                // Control may have changed while the confirmation was open.
                // Re-check at the destructive boundary so a stale prompt cannot
                // kill active work.
                if harnesses.control(id) == Some(crate::worker::pty::SessionControl::Orchestrator) {
                    self.set_status("Task sessions are view-only while they are running");
                    return;
                }
                if !harnesses.sessions.close(id) {
                    self.set_status("That session is gone");
                    return;
                }
            }
            SessionRef::Remote { host, session } => {
                let session = session.to_string();
                let Some(state) = self.remote_hosts.get_mut(host) else {
                    self.set_status("That host is not connected");
                    return;
                };
                if let Err(reason) =
                    state.request(crate::remote::client::RemoteRequest::Close(session))
                {
                    self.set_status(format!("Could not close it: {reason}"));
                    return;
                }
            }
        }
        if self.harness_focus.is_attached_to(session) {
            self.release_session();
        }
        self.set_status("Closed the harness");
    }
}
