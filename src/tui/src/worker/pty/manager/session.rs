//! Session bookkeeping: liveness, claiming, and the rows callers read.
//!
//! Every method here that can block is the same shape: resolve the handle, drop
//! the registry lock, act on the session. The two that cannot — building rows
//! and counting live sessions — read under the lock, since they touch nothing
//! but atomics and run every frame.

use medulla::protocol::HarnessProvider;

use super::super::types::{HarnessControl, SessionRow};
use super::{read, write, PtyManager};

/// Advance the consumed-bell watermark without allowing a stale screen sample
/// to move it backwards.
pub(super) fn consumed_bell_count(seen: usize, sampled: usize) -> usize {
    seen.max(sampled)
}

impl PtyManager {
    /// Take an idle session for `label` on `provider`, marking it busy.
    ///
    /// The claim is a compare-exchange on the session's own `busy` flag, so it
    /// is atomic without holding the registry: two concurrent tasks that both
    /// see the same idle session cannot both take it, because only one CAS can
    /// win and the loser simply keeps looking. That collision is precisely what
    /// this exists to prevent, and it only shows up under a real fan-out.
    ///
    /// A session the operator holds is never returned, however idle it looks.
    /// That is the whole of the unmanaged-harness feature and the whole of
    /// takeover: [`HarnessControl`] is the gate, and this is where it bites.
    /// Without it, attaching to a pane and typing does not stop the orchestrator
    /// pasting a task prompt into the same composer — the exact two-writers
    /// collision the `busy` flag above exists to prevent, reachable by an
    /// operator simply focusing a harness.
    ///
    /// A handed-back operator session still has its synthetic `you:<provider>`
    /// label. The first real conversation to claim it adopts that conversation
    /// label, making subsequent reuse obey the same exact-label rule as every
    /// task-spawned session.
    ///
    /// `None` when there is no idle session, and the caller opens a fresh one.
    pub fn claim_idle(&self, label: &str, provider: HarnessProvider) -> Option<SessionRow> {
        let session = self
            .handles()
            .into_iter()
            .filter(|session| session.provider() == provider && session.serves_label(label))
            .find(|session| session.try_claim())?;
        // After the claim, never before: the winner of the compare-exchange is
        // the one task entitled to rename the session, so adoption inherits the
        // claim's exclusivity instead of needing a lock of its own.
        session.adopt_label(label);
        Some(session.row())
    }

    /// The operator-held session covering `cwd`, if there is one.
    ///
    /// The half of exclusivity [`claim_idle`](Self::claim_idle) cannot express.
    /// `claim_idle` answers "may I reuse *this* session", which a caller can walk
    /// straight past by opening a second harness in the same folder — and did:
    /// taking over the only harness in a workspace made the next task frame spawn
    /// a rival process in the same working tree, two agents editing one repo with
    /// no mutual exclusion at all. This answers the question that actually
    /// governs: "may anything start here right now".
    ///
    /// A workspace with a person in it is not shared, however idle another
    /// harness in it looks, so this is consulted *before* reuse rather than after.
    ///
    /// Walks a cloned list of handles rather than holding the registry, because
    /// `same_workspace` canonicalizes both paths and that is a filesystem call —
    /// exactly the blocking work no caller may hold the registry across.
    pub fn operator_hold(&self, cwd: &str) -> Option<SessionRow> {
        self.handles()
            .into_iter()
            .find(|session| {
                session.control() == HarnessControl::User
                    && session.is_running()
                    && same_workspace(session.cwd(), cwd)
            })
            .map(|session| session.row())
    }

    /// Who currently holds `id`, if it is a session we know about.
    pub fn control(&self, id: &str) -> Option<HarnessControl> {
        self.handle(id).map(|session| session.control())
    }

    /// Hand `id` to `control`, returning whether the session existed.
    ///
    /// Deliberately leaves `busy` alone. The two flags answer different
    /// questions — `busy` is "is a turn running in it", `control` is "who is
    /// allowed to start one" — and a session taken over mid-turn is still
    /// running that turn. Clearing `busy` on handback would advertise a harness
    /// as free while it was still finishing someone else's work.
    pub fn set_control(&self, id: &str, control: HarnessControl) -> bool {
        let Some(session) = self.handle(id) else {
            return false;
        };
        session.set_control(control);
        true
    }

    /// Mark a session free without implying that a turn was submitted.
    ///
    /// Used for failed injection and claim rollback, where no completion chime
    /// can be pending and the next bell must remain meaningful.
    pub fn release(&self, id: &str) {
        if let Some(session) = self.handle(id) {
            session.release();
        }
    }

    /// Mark a submitted turn complete and suppress its completion chime.
    pub fn settle_turn(&self, id: &str) {
        if let Some(session) = self.handle(id) {
            session.settle_turn();
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

    /// Whether the named session's child environment selects a GitHub repository.
    pub(in crate::worker) fn gh_repo_is_set(&self, id: &str) -> Option<bool> {
        self.handle(id).map(|session| session.gh_repo_is_set())
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

    /// Interrupt and close a session only if the operator has not taken it.
    pub fn stop_if_orchestrator(&self, id: &str) -> bool {
        self.handle(id)
            .is_some_and(|session| session.stop_if_orchestrator())
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

/// Whether two paths name the same working directory.
///
/// String equality is not enough here because the two sides arrive by different
/// routes: a harness the operator started ran its path through `expand_home`,
/// while a task frame's `cwd` is used verbatim as the orchestrator wrote it. So
/// `~/work/repo` and `/Users/e/work/repo` are one workspace that compares as
/// two — and exclusivity that can be defeated by how a path was spelled is not
/// exclusivity.
///
/// Canonicalizes both sides, which also resolves symlinks (on macOS `/tmp` is a
/// link to `/private/tmp`, so this is load-bearing in tests as well as in the
/// field). Falls back to a trimmed textual compare when either path cannot be
/// resolved — a directory that does not exist yet is still worth matching by
/// name, and failing to canonicalize must not silently drop the hold.
fn same_workspace(a: &str, b: &str) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a.trim_end_matches('/') == b.trim_end_matches('/'),
    }
}
