//! Session bookkeeping: liveness, claiming, and the rows callers read.

use medulla::tinyplace::HarnessProvider;

use super::super::attention::AttentionKind;
use super::super::types::{HarnessControl, PtyState, SessionRow};

use super::PtyManager;

/// Advance the consumed-bell watermark without allowing a stale screen sample
/// to move it backwards.
pub(super) fn consumed_bell_count(seen: usize, sampled: usize) -> usize {
    seen.max(sampled)
}

impl PtyManager {
    /// Record why a queued write never reached the child.
    ///
    /// The write half is drained by a thread (see [`PtyManager`]'s
    /// `spawn_writer`), so a failed write has no caller to return to — this row
    /// field is where it is kept instead. Last-one-wins: the interesting failure
    /// is the current one, and a session whose pty has stopped accepting bytes
    /// will not recover to produce a different one.
    pub(super) fn record_write_error(&self, id: &str, error: &str) {
        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) {
            session.row.last_error = Some(error.to_string());
        }
    }

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
        let mut sessions = self.inner.sessions.lock().unwrap();
        let session = sessions.iter_mut().find(|s| {
            (s.row.label == label || is_unassigned_operator_session(&s.row))
                && s.row.provider == provider
                && s.row.state.is_running()
                && !s.row.busy
                && s.row.control.is_orchestrator()
        })?;
        if is_unassigned_operator_session(&session.row) {
            session.row.label = label.to_string();
        }
        session.row.busy = true;
        session.suppress_next_bell = false;
        Some(session.row.clone())
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
    pub fn operator_hold(&self, cwd: &str) -> Option<SessionRow> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .find(|s| {
                s.row.control == HarnessControl::User
                    && s.row.state.is_running()
                    && same_workspace(&s.row.cwd, cwd)
            })
            .map(|s| s.row.clone())
    }

    /// Who currently holds `id`, if it is a session we know about.
    pub fn control(&self, id: &str) -> Option<HarnessControl> {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.row.id == id)
            .map(|s| s.row.control)
    }

    /// Hand `id` to `control`, returning whether the session existed.
    ///
    /// Deliberately leaves `busy` alone. The two flags answer different
    /// questions — `busy` is "is a turn running in it", `control` is "who is
    /// allowed to start one" — and a session taken over mid-turn is still
    /// running that turn. Clearing `busy` on handback would advertise a harness
    /// as free while it was still finishing someone else's work.
    pub fn set_control(&self, id: &str, control: HarnessControl) -> bool {
        let mut sessions = self.inner.sessions.lock().unwrap();
        let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) else {
            return false;
        };
        session.row.control = control;
        true
    }

    /// Mark a session free for the next turn.
    ///
    /// A bell heard as the turn settled is a completion chime, not a durable
    /// request for the operator. Consume the emulator's current bell count and
    /// clear that fallback cue while retaining named prompts, which still
    /// describe something visible on the reusable screen. Bumping the attention
    /// generation also prevents a classifier already reading the screen from
    /// restoring the completed turn's bell after this method returns.
    pub fn release(&self, id: &str) {
        let bells = self.current_bell_count(id);

        let mut sessions = self.inner.sessions.lock().unwrap();
        if let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) {
            session.row.busy = false;
            if let Some(bells) = bells {
                // A classifier may have stored a newer count after the screen
                // snapshot above. Bell counters are monotonic, so never move
                // the consumed watermark backwards.
                session.seen_bells = consumed_bell_count(session.seen_bells, bells);
            }
            session.attention_generation = session.attention_generation.wrapping_add(1);
            session.suppress_next_bell = true;
            session.row.attention = session
                .row
                .attention
                .take()
                .filter(|cue| cue.kind != AttentionKind::Bell);
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

/// Whether an operator-spawned session has not yet served a real conversation.
fn is_unassigned_operator_session(row: &SessionRow) -> bool {
    row.user_spawned && row.label.starts_with("you:")
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
