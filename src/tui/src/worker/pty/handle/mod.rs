//! [`SessionHandle`] — one live PTY session's own state and its own locks.
//!
//! Everything the manager can do to a single session lives here, so the manager
//! itself is only a registry: find the handle, drop the registry lock, call a
//! method. See [`types`] for why the state is split the way it is.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use medulla::tinyplace::HarnessProvider;
use portable_pty::{Child, MasterPty, PtySize};

use super::sync::lock;
use super::types::{HarnessControl, PtyState, SessionRow};

mod screen;
mod types;

pub(crate) use types::{ColdFields, SessionIo, SessionMeta};
pub use types::{SessionHandle, MAX_QUEUED_WRITE_BYTES};
use types::{NO_EXIT_CODE, STATE_EXITED, STATE_FAILED, STATE_RUNNING};

impl SessionHandle {
    /// Build a handle for a child that has just been spawned.
    ///
    /// `writes` is the sending half of the queue the session's writer thread
    /// drains, and `queued_bytes` the budget they share; see
    /// [`SessionIo::writes`].
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        meta: SessionMeta,
        label: String,
        session_id: Option<String>,
        control: HarnessControl,
        screen: vt100::Parser,
        master: Box<dyn MasterPty + Send>,
        writes: Sender<Vec<u8>>,
        queued_bytes: Arc<AtomicUsize>,
        child: Box<dyn Child + Send + Sync>,
    ) -> Self {
        let started_at = meta.started_at;
        // Opened because a turn is about to run in it. Claimed here so a
        // concurrent task cannot take it in the gap before that turn starts.
        //
        // Not so for an operator-spawned session: nothing is about to run in it,
        // it is sitting at a prompt waiting to be typed in. Leaving it claimed
        // would make the rail read "busy" for a harness that is plainly idle.
        let busy = !meta.user_spawned;
        SessionHandle {
            meta,
            state: AtomicU8::new(STATE_RUNNING),
            exit_code: AtomicI64::new(NO_EXIT_CODE),
            last_output_at: AtomicI64::new(started_at),
            generation: AtomicU64::new(0),
            busy: AtomicBool::new(busy),
            operator_held: AtomicBool::new(control == HarnessControl::User),
            queued_bytes,
            cold: Mutex::new(ColdFields {
                label,
                session_id,
                last_error: None,
            }),
            screen: Mutex::new(screen),
            io: Mutex::new(Some(SessionIo { master, writes })),
            child: Mutex::new(Some(child)),
        }
    }

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
    pub(super) fn mark_output(&self, now: i64) {
        self.last_output_at.store(now, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Whether a turn is running in this session right now.
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    /// Who holds this session right now.
    pub fn control(&self) -> HarnessControl {
        if self.operator_held.load(Ordering::Acquire) {
            HarnessControl::User
        } else {
            HarnessControl::Orchestrator
        }
    }

    /// Hand the session to `control`.
    ///
    /// Deliberately leaves `busy` alone. The two flags answer different
    /// questions — `busy` is "is a turn running in it", control is "who is
    /// allowed to start one" — and a session taken over mid-turn is still
    /// running that turn. Clearing `busy` on handback would advertise a harness
    /// as free while it was still finishing someone else's work.
    pub(super) fn set_control(&self, control: HarnessControl) {
        self.operator_held
            .store(control == HarnessControl::User, Ordering::Release);
    }

    /// Whether this session may serve `label`'s next turn.
    ///
    /// Its own label, or the synthetic `you:<harness>` one of a handed-back
    /// operator session that has not served a real conversation yet — see
    /// [`adopt_label`](Self::adopt_label).
    pub(super) fn serves_label(&self, label: &str) -> bool {
        let cold = lock(&self.cold);
        cold.label == label || (self.meta.user_spawned && cold.label.starts_with("you:"))
    }

    /// Adopt `label` if this is a handed-back operator session still carrying
    /// its synthetic one.
    ///
    /// Called by the winner of [`try_claim`](Self::try_claim), so the rename
    /// happens once and under the same claim that made it exclusive. After it,
    /// reuse obeys the same exact-label rule as every task-spawned session —
    /// adoption must not turn one harness into a cross-conversation pool.
    pub(super) fn adopt_label(&self, label: &str) {
        let mut cold = lock(&self.cold);
        if self.meta.user_spawned && cold.label.starts_with("you:") {
            cold.label = label.to_string();
        }
    }

    /// Take this session for a turn, if it is free, alive, and the
    /// orchestrator's to take.
    ///
    /// A single compare-exchange, which is what makes the claim atomic without
    /// the registry lock the old find-and-claim needed: two tasks racing for the
    /// same idle session cannot both win, because only one CAS can succeed.
    ///
    /// A session the operator holds is never claimed, however idle it looks.
    /// That is the whole of the unmanaged-harness feature and the whole of
    /// takeover: without it, attaching to a pane and typing does not stop the
    /// orchestrator pasting a task prompt into the same composer — the exact
    /// two-writers collision `busy` exists to prevent, reachable by an operator
    /// simply focusing a harness.
    /// Control is checked again *after* the claim lands, and the claim given
    /// back if it changed: an operator who takes a harness in the window between
    /// the two must win it, because the alternative is a prompt pasted into
    /// their composer a moment after they started typing.
    pub(super) fn try_claim(&self) -> bool {
        if !self.is_running() || !self.control().is_orchestrator() {
            return false;
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        if self.control().is_orchestrator() {
            return true;
        }
        self.release();
        false
    }

    /// Mark the session free for the next turn.
    pub(super) fn release(&self) {
        self.busy.store(false, Ordering::Release);
    }

    /// The harness session id, once known.
    pub fn session_id(&self) -> Option<String> {
        lock(&self.cold).session_id.clone()
    }

    /// Record the harness session id a tailer read back from the rollout.
    ///
    /// First writer wins: claude's is minted at spawn and never changes, so this
    /// only ever fills in codex's.
    pub(super) fn record_session_id(&self, harness_session_id: String) {
        let mut cold = lock(&self.cold);
        if cold.session_id.is_none() {
            cold.session_id = Some(harness_session_id);
        }
    }

    /// Record why a queued write never reached the child.
    ///
    /// The write half is drained by a thread (see the manager's `spawn_writer`),
    /// so a failed write has no caller to return to — this row field is where it
    /// is kept instead. Last-one-wins: the interesting failure is the current
    /// one, and a session whose pty has stopped accepting bytes will not recover
    /// to produce a different one.
    pub(super) fn record_error(&self, error: String) {
        lock(&self.cold).last_error = Some(error);
    }

    /// The operator-facing projection of this session, for the list pane.
    pub fn row(&self) -> SessionRow {
        let cold = lock(&self.cold);
        SessionRow {
            id: self.meta.id.clone(),
            label: cold.label.clone(),
            provider: self.meta.provider,
            state: self.state(),
            cwd: self.meta.cwd.clone(),
            branch: self.meta.branch.clone(),
            session_id: cold.session_id.clone(),
            started_at: self.meta.started_at,
            last_output_at: self.last_output_at(),
            last_error: cold.last_error.clone(),
            busy: self.is_busy(),
            control: self.control(),
            user_spawned: self.meta.user_spawned,
        }
    }

    /// Ask the child to die. Does not wait for it.
    pub(super) fn kill(&self) {
        if let Some(child) = lock(&self.child).as_mut() {
            let _ = child.kill();
        }
    }

    /// Note that the session has ended, without waiting on the child.
    ///
    /// What `close` does: the reader thread sees the pty close a moment later
    /// and [`reap`](SessionHandle::reap) settles the real status.
    pub(super) fn mark_closed(&self) {
        let _ = self.state.compare_exchange(
            STATE_RUNNING,
            STATE_EXITED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Wait on the child, record its status, and release its resources.
    ///
    /// Called from the reader thread once the pty master closes. The child is
    /// taken out and waited on with **no lock held that anyone else wants** —
    /// EOF on the master and the child's exit are not simultaneous, so `wait()`
    /// can block for a moment, and this used to happen under the manager's one
    /// global lock, which showed up as the whole TUI freezing when a session
    /// ended.
    ///
    /// The pty ends are dropped here too. They are file descriptors, and nothing
    /// reads or writes them once the child is gone, but they used to be held
    /// until an operator pressed the key that forgot the session — so a worker
    /// serving a long fan-out leaked a pair per finished task until it could not
    /// open any more. Dropping them also drops the write queue's sender, which
    /// is what ends the writer thread rather than leaving it parked on a receive
    /// nothing will ever satisfy.
    pub(super) fn reap(&self, now: i64) {
        let child = lock(&self.child).take();
        let code = child
            .and_then(|mut child| child.wait().ok())
            .map(|status| status.exit_code() as i32);

        self.exit_code
            .store(code.map_or(NO_EXIT_CODE, i64::from), Ordering::Release);
        self.state.store(STATE_EXITED, Ordering::Release);
        self.last_output_at.store(now, Ordering::Release);
        // The screen is deliberately kept: the operator usually wants to read
        // how a session ended. Only the descriptors go.
        *lock(&self.io) = None;
    }

    /// Queue raw bytes for the pty — the focused pane's keystrokes, and the
    /// prompts [`inject_prompt`](super::inject_prompt) pastes.
    ///
    /// Returns as soon as the bytes are queued; the session's writer thread
    /// performs the write. Nothing here blocks, and that is the point twice
    /// over. `write_all` on a pty master **parks in the kernel** when the child
    /// is not draining its input — mid tool call, stopped on a modal, or simply
    /// still loading — and the tty buffer is only a few kilobytes, which an
    /// injected prompt can exceed on its own. Giving every session its own locks
    /// stops that stalling *other* sessions; handing the write to a thread stops
    /// it stalling the *caller*, which on the keystroke path is the render
    /// thread.
    ///
    /// Queueing cannot block, so the queue is bounded by *refusal* instead:
    /// [`MAX_QUEUED_WRITE_BYTES`] of unwritten bytes per session, past which a
    /// write is rejected rather than waited out. A child that never drains its
    /// stdin therefore cannot make this grow without limit.
    ///
    /// # Errors
    ///
    /// When the session has exited, when `bytes` alone exceeds the queue, when
    /// the queue is already full, or when the writer thread is gone. A failure
    /// of the *write itself* has no caller left to reach and is recorded on the
    /// row's `last_error` instead.
    pub(super) fn write(&self, bytes: &[u8]) -> Result<(), String> {
        // Refused before anything is copied: a write larger than the whole budget
        // can never be admitted, however empty the queue is, and saying so here
        // beats reserving and unwinding.
        if bytes.len() > MAX_QUEUED_WRITE_BYTES {
            return Err(format!(
                "{}: {} bytes is more than the {MAX_QUEUED_WRITE_BYTES}-byte write queue holds",
                self.meta.id,
                bytes.len()
            ));
        }
        if !self.is_running() {
            return Err(format!("{} has exited", self.meta.id));
        }
        // The queue handle is cloned out and the guard dropped *before* the send,
        // so even this session's own lock is held for a clone and nothing more.
        let writes = {
            let io = lock(&self.io);
            let Some(io) = io.as_ref() else {
                return Err(format!("{} has exited", self.meta.id));
            };
            io.writes.clone()
        };
        // Reserved before queueing, and atomically, so two concurrent writers
        // cannot both observe room and both take it.
        if self
            .queued_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                let wanted = queued + bytes.len();
                (wanted <= MAX_QUEUED_WRITE_BYTES).then_some(wanted)
            })
            .is_err()
        {
            return Err(format!(
                "{}: the write queue is full ({MAX_QUEUED_WRITE_BYTES} bytes unwritten) — \
                 the harness is not reading its input",
                self.meta.id
            ));
        }
        // Fails only if the writer thread has stopped, which means the pty stopped
        // accepting bytes. Worth reporting: the caller's keystrokes went nowhere.
        writes.send(bytes.to_vec()).map_err(|_| {
            self.queued_bytes.fetch_sub(bytes.len(), Ordering::AcqRel);
            format!("{}: the writer thread is gone", self.meta.id)
        })
    }

    /// Resize the pty and the emulator together.
    ///
    /// Both must move: the child reflows to the pty size, so an emulator of a
    /// different size renders a torn screen.
    pub(super) fn resize(&self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        {
            let mut parser = lock(&self.screen);
            if parser.screen().size() == (rows, cols) {
                return; // already correct — skip the SIGWINCH storm
            }
            parser.set_size(rows, cols);
        }
        if let Some(io) = lock(&self.io).as_ref() {
            let _ = io.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }
}
