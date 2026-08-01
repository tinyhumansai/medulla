//! Session bookkeeping: liveness, claiming, and the rows callers read.
//!
//! Every method here that can block is the same shape: resolve the handle, drop
//! the registry lock, act on the session. The two that cannot — building rows
//! and counting live sessions — read under the lock, since they touch nothing
//! but atomics and run every frame.

use medulla::tinyplace::HarnessProvider;

use super::super::types::SessionRow;
use super::{read, write, PtyManager};

impl PtyManager {
    /// Take an idle session for `label` on `provider`, marking it busy.
    ///
    /// The claim is a compare-exchange on the session's own `busy` flag, so it
    /// is atomic without holding the registry: two concurrent tasks that both
    /// see the same idle session cannot both take it, because only one CAS can
    /// win and the loser simply keeps looking. That collision is precisely what
    /// this exists to prevent, and it only shows up under a real fan-out.
    ///
    /// `None` when there is no idle session, and the caller opens a fresh one.
    pub fn claim_idle(&self, label: &str, provider: HarnessProvider) -> Option<SessionRow> {
        self.handles()
            .into_iter()
            .filter(|session| session.label() == label && session.provider() == provider)
            .find(|session| session.try_claim())
            .map(|session| session.row())
    }

    /// Mark a session free for the next turn.
    pub fn release(&self, id: &str) {
        if let Some(session) = self.handle(id) {
            session.release();
        }
    }

    /// Every session, open order — the list pane's rows.
    ///
    /// Built under the registry's read lock rather than off a cloned `Arc` list,
    /// which is the one place that is worth doing: this runs every frame, and
    /// building a row touches only atomics and one small per-session mutex —
    /// never a pty, a child, or an emulator. The rule the split enforces is that
    /// nothing holds the registry across *blocking* work, and none of this
    /// blocks. Read locks do not exclude each other, so concurrent renders and
    /// samplers still proceed in parallel.
    pub fn rows(&self) -> Vec<SessionRow> {
        read(&self.inner.sessions)
            .iter()
            .map(|session| session.row())
            .collect()
    }

    /// One session's row by id.
    pub fn row(&self, id: &str) -> Option<SessionRow> {
        self.handle(id).map(|session| session.row())
    }

    /// How many sessions are still running.
    ///
    /// One relaxed atomic load per session, where it used to be a global lock
    /// and a full scan — and it is asked on every render frame.
    pub fn running_count(&self) -> usize {
        read(&self.inner.sessions)
            .iter()
            .filter(|session| session.is_running())
            .count()
    }

    /// The screen change counter for `id`.
    ///
    /// Bumped whenever the emulator consumes output, so a caller that remembers
    /// what it last rendered can skip rebuilding a snapshot for a screen that
    /// has not moved.
    pub fn generation(&self, id: &str) -> Option<u64> {
        self.handle(id).map(|session| session.generation())
    }

    /// Record the harness session id a tailer read back from the rollout.
    ///
    /// Codex cannot be told an id, so its own is only knowable once it has
    /// written line one of its rollout. Claude's is minted at spawn and never
    /// changes, so this is a no-op there.
    pub fn record_session_id(&self, id: &str, harness_session_id: impl Into<String>) {
        if let Some(session) = self.handle(id) {
            session.record_session_id(harness_session_id.into());
        }
    }

    /// Ask a session's harness to exit, then reap it.
    ///
    /// Sends the child a kill rather than typing `/exit`: the harnesses disagree
    /// on the command, and a session the operator asked to close should not
    /// depend on the model cooperating.
    ///
    /// The exit status is settled by the reader thread when the pty closes a
    /// moment later; this only marks the session as no longer live, so nothing
    /// claims it in the meantime.
    pub fn close(&self, id: &str) -> bool {
        let Some(session) = self.handle(id) else {
            return false;
        };
        session.kill();
        session.mark_closed();
        true
    }

    /// Drop an exited session's record and screen.
    ///
    /// Refuses while the child is alive, so a forgotten session can never leave
    /// an orphaned process holding a PTY.
    pub fn forget(&self, id: &str) -> bool {
        let mut sessions = write(&self.inner.sessions);
        let Some(index) = sessions
            .iter()
            .position(|session| session.id() == id && !session.is_running())
        else {
            return false;
        };
        sessions.remove(index);
        true
    }

    /// Kill every child. Called on shutdown so no harness outlives the TUI.
    pub fn shutdown(&self) {
        for session in self.handles() {
            session.kill();
        }
    }
}
