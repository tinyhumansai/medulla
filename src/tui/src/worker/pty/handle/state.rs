//! Read-mostly accessors and the operator-facing row projection.

use std::sync::atomic::Ordering;

use medulla::protocol::HarnessProvider;

use super::super::sync::lock;
use super::super::types::{PtyState, SessionRow};
use super::types::{NO_EXIT_CODE, STATE_FAILED, STATE_RUNNING};
use super::SessionHandle;

impl SessionHandle {
    /// The manager's stable local id.
    pub fn id(&self) -> &str {
        &self.meta.id
    }

    /// The list label — the peer's id, or `you:<harness>` for one an operator
    /// started.
    pub fn label(&self) -> String {
        lock(&self.cold).label.clone()
    }

    /// The working directory the child runs in.
    pub fn cwd(&self) -> &str {
        &self.meta.cwd
    }

    /// Which harness is running.
    pub fn provider(&self) -> HarnessProvider {
        self.meta.provider
    }

    /// Where the session is in its life.
    ///
    /// One relaxed load and, when it has ended, one more for the status — no
    /// lock, because this is asked on every render frame for every row.
    pub fn state(&self) -> PtyState {
        match self.state.load(Ordering::Acquire) {
            STATE_RUNNING => PtyState::Running,
            STATE_FAILED => PtyState::Failed,
            _ => PtyState::Exited {
                code: match self.exit_code.load(Ordering::Acquire) {
                    NO_EXIT_CODE => None,
                    code => Some(code as i32),
                },
            },
        }
    }

    /// Whether the child is still alive.
    pub fn is_running(&self) -> bool {
        self.state.load(Ordering::Acquire) == STATE_RUNNING
    }

    /// The screen's change counter. See [`SessionHandle::generation`].
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Epoch ms of the last output byte.
    pub fn last_output_at(&self) -> i64 {
        self.last_output_at.load(Ordering::Acquire)
    }

    /// Record that the emulator consumed output at `now`.
    ///
    /// The whole of what the reader thread has to do per read, and it is two
    /// atomic stores against a handle it already holds — where it used to be a
    /// process-wide mutex plus a linear scan of every session, per 8 KB read,
    /// on every reader thread at once.
    pub(in super::super) fn mark_output(&self, now: i64) {
        self.last_output_at.store(now, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Whether a turn is running in this session right now.
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    /// The harness session id, once known.
    pub fn session_id(&self) -> Option<String> {
        lock(&self.cold).session_id.clone()
    }

    /// The key this session's MCP grant was minted under, when one was.
    ///
    /// Immutable, so it is readable without touching either lock — which is
    /// what lets the manager reach the control plane after releasing its own.
    pub(in super::super) fn grant_session(&self) -> Option<&str> {
        self.meta.mcp_grant_session.as_deref()
    }

    /// Whether this session's child environment selects a GitHub repository.
    pub(in super::super) fn gh_repo_is_set(&self) -> bool {
        self.meta.gh_repo_is_set
    }

    /// Record the harness session id a tailer read back from the rollout.
    ///
    /// First writer wins: claude's is minted at spawn and never changes, so this
    /// only ever fills in codex's.
    pub(in super::super) fn record_session_id(&self, harness_session_id: String) {
        let mut cold = lock(&self.cold);
        if cold.session_id.is_none() {
            cold.session_id = Some(harness_session_id);
        }
    }

    /// Give this session the display name a person chose for it.
    ///
    /// Naming is not ownership and not provenance: renaming a session neither
    /// takes it (that is [`set_control`](Self::set_control)) nor changes who
    /// started it (that is fixed at birth). A blank name clears it back to
    /// unnamed rather than storing an empty label the rail would render as a
    /// gap.
    pub(in super::super) fn set_name(&self, name: Option<String>) {
        lock(&self.cold).name = name.filter(|name| !name.trim().is_empty());
    }

    /// Record a freshly read checkout, reporting whether anything moved.
    ///
    /// The answer is compared rather than stored blindly so the poller can stay
    /// silent while nothing changes: a redraw per session per tick, for four
    /// sessions that are all still on the branch they started on, is a cost
    /// paid for no new information.
    pub(in super::super) fn set_checkout(&self, checkout: medulla::ui::checkout::Checkout) -> bool {
        let mut cold = lock(&self.cold);
        if cold.checkout == checkout {
            return false;
        }
        cold.checkout = checkout;
        true
    }

    /// Record why a queued write never reached the child.
    ///
    /// The write half is drained by a thread (see the manager's `spawn_writer`),
    /// so a failed write has no caller to return to — this row field is where it
    /// is kept instead. Last-one-wins: the interesting failure is the current
    /// one, and a session whose pty has stopped accepting bytes will not recover
    /// to produce a different one.
    pub(in super::super) fn record_error(&self, error: String) {
        lock(&self.cold).last_error = Some(error);
    }

    /// Record the thread name discovered outside the terminal stream.
    ///
    /// Codex persists renamed threads in its session index instead of emitting
    /// an OSC window title, so the transcript executor supplies that value once
    /// it has identified the session.
    pub(in super::super) fn record_thread_name(&self, thread_name: String) {
        lock(&self.cold).thread_name = Some(thread_name);
    }

    /// The operator-facing projection of this session, for the list pane.
    pub fn row(&self) -> SessionRow {
        let cold = lock(&self.cold);
        SessionRow {
            id: self.meta.id.clone(),
            label: cold.label.clone(),
            provider: self.meta.provider,
            preset: self.meta.preset.clone(),
            state: self.state(),
            cwd: self.meta.cwd.clone(),
            checkout: cold.checkout.clone(),
            launch_root: self.meta.launch_root.clone(),
            launch_commit: self.meta.launch_commit.clone(),
            launch_checkout_identity: self.meta.launch_checkout_identity.clone(),
            session_id: cold.session_id.clone(),
            thread_name: cold.thread_name.clone(),
            started_at: self.meta.started_at,
            last_output_at: self.last_output_at(),
            last_error: cold.last_error.clone(),
            busy: self.is_busy(),
            control: self.control(),
            origin: self.meta.origin,
            retained: self.is_retained(),
            name: cold.name.clone(),
            attention: lock(&self.attention).cue.clone(),
            mcp_grant_session: self.meta.mcp_grant_session.clone(),
        }
    }
}
