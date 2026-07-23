//! Session bookkeeping: liveness, claiming, and the rows callers read.

use medulla::tinyplace::HarnessProvider;

use super::super::types::{PtyState, SessionRow};

use super::PtyManager;

impl PtyManager {
    /// Record that a session produced output.
    pub(super) fn touch(&self, id: &str) {
        let now = self.now();
        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) {
            session.row.last_output_at = now;
        }
    }

    /// Reap a session whose PTY has closed and record its exit status.
    ///
    /// The child is taken out of the record and waited on with the lock
    /// **released**. EOF on the pty master and the child's exit are not
    /// simultaneous, so `wait()` can block for a moment — and holding the
    /// manager's lock across it stalls every render frame, which shows up as the
    /// whole TUI freezing when a session ends.
    pub(super) fn mark_finished(&self, id: &str) {
        let child = {
            let mut sessions = self.inner.sessions.lock().unwrap();
            match sessions.iter_mut().find(|s| s.row.id == id) {
                Some(session) => session.child.take(),
                None => return,
            }
        };
        let code = child
            .and_then(|mut child| child.wait().ok())
            .map(|status| status.exit_code() as i32);

        let now = self.now();
        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) {
            session.row.state = PtyState::Exited { code };
            session.row.last_output_at = now;
        }
    }

    /// Every session, open order — the list pane's rows.
    /// Take an idle session for `label` on `provider`, marking it busy.
    ///
    /// Find-and-claim under one lock, deliberately. Checking `busy` and then
    /// setting it in two steps lets two concurrent tasks both observe the same
    /// idle session and both take it — which is precisely the collision this
    /// exists to prevent, and it would show up only under a real fan-out.
    ///
    /// `None` when there is no idle session, and the caller opens a fresh one.
    pub fn claim_idle(&self, label: &str, provider: HarnessProvider) -> Option<SessionRow> {
        let mut sessions = self.inner.sessions.lock().unwrap();
        let session = sessions.iter_mut().find(|s| {
            s.row.label == label
                && s.row.provider == provider
                && s.row.state.is_running()
                && !s.row.busy
        })?;
        session.row.busy = true;
        Some(session.row.clone())
    }

    /// Mark a session free for the next turn.
    pub fn release(&self, id: &str) {
        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) {
            session.row.busy = false;
        }
    }

    pub fn rows(&self) -> Vec<SessionRow> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.row.clone())
            .collect()
    }

    /// One session's row by id.
    pub fn row(&self, id: &str) -> Option<SessionRow> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.row.id == id)
            .map(|s| s.row.clone())
    }

    /// How many sessions are still running.
    pub fn running_count(&self) -> usize {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.row.state.is_running())
            .count()
    }

    /// Record the harness session id a tailer read back from the rollout.
    ///
    /// Codex cannot be told an id, so its own is only knowable once it has
    /// written line one of its rollout. Claude's is minted at spawn and never
    /// changes, so this is a no-op there.
    pub fn record_session_id(&self, id: &str, harness_session_id: impl Into<String>) {
        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) {
            if session.row.session_id.is_none() {
                session.row.session_id = Some(harness_session_id.into());
            }
        }
    }

    /// Ask a session's harness to exit, then reap it.
    ///
    /// Sends the child a kill rather than typing `/exit`: the harnesses disagree
    /// on the command, and a session the operator asked to close should not
    /// depend on the model cooperating.
    pub fn close(&self, id: &str) -> bool {
        let mut sessions = self.inner.sessions.lock().unwrap();
        let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) else {
            return false;
        };
        if let Some(child) = session.child.as_mut() {
            let _ = child.kill();
        }
        session.row.state = PtyState::Exited { code: None };
        true
    }

    /// Drop an exited session's record and screen.
    ///
    /// Refuses while the child is alive, so a forgotten session can never leave
    /// an orphaned process holding a PTY.
    pub fn forget(&self, id: &str) -> bool {
        let mut sessions = self.inner.sessions.lock().unwrap();
        let Some(index) = sessions
            .iter()
            .position(|s| s.row.id == id && !s.row.state.is_running())
        else {
            return false;
        };
        sessions.remove(index);
        true
    }

    /// Kill every child. Called on shutdown so no harness outlives the TUI.
    pub fn shutdown(&self) {
        let mut sessions = self.inner.sessions.lock().unwrap();
        for session in sessions.iter_mut() {
            if let Some(child) = session.child.as_mut() {
                let _ = child.kill();
            }
        }
    }
}
