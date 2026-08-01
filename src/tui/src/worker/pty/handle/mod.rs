//! [`SessionHandle`] — one live PTY session's own state and its own locks.
//!
//! Everything the manager can do to a single session lives here, so the manager
//! itself is only a registry: find the handle, drop the registry lock, call a
//! method. See [`types`] for why the state is split the way it is.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;

use medulla::tinyplace::HarnessProvider;
use portable_pty::{Child, MasterPty, PtySize};

use super::sync::lock;
use super::types::{PtyState, SessionRow};

mod screen;
mod types;

pub use types::SessionHandle;
pub(crate) use types::{ColdFields, SessionIo, SessionMeta};
use types::{NO_EXIT_CODE, STATE_EXITED, STATE_FAILED, STATE_RUNNING};

impl SessionHandle {
    /// Build a handle for a child that has just been spawned.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        meta: SessionMeta,
        session_id: Option<String>,
        screen: vt100::Parser,
        master: Box<dyn MasterPty + Send>,
        writer: Box<dyn std::io::Write + Send>,
        child: Box<dyn Child + Send + Sync>,
    ) -> Self {
        let started_at = meta.started_at;
        SessionHandle {
            meta,
            state: AtomicU8::new(STATE_RUNNING),
            exit_code: AtomicI64::new(NO_EXIT_CODE),
            last_output_at: AtomicI64::new(started_at),
            generation: AtomicU64::new(0),
            // Opened because a turn is about to run in it. Claimed here so a
            // concurrent task cannot take it in the gap before that turn starts.
            busy: AtomicBool::new(true),
            cold: Mutex::new(ColdFields {
                session_id,
                last_error: None,
            }),
            screen: Mutex::new(screen),
            io: Mutex::new(Some(SessionIo { master, writer })),
            child: Mutex::new(Some(child)),
        }
    }

    /// The manager's stable local id.
    pub fn id(&self) -> &str {
        &self.meta.id
    }

    /// The list label — the peer's id.
    pub fn label(&self) -> &str {
        &self.meta.label
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

    /// Take this session for a turn, if it is free and alive.
    ///
    /// A single compare-exchange, which is what makes the claim atomic without
    /// the registry lock the old find-and-claim needed: two tasks racing for the
    /// same idle session cannot both win, because only one CAS can succeed.
    pub(super) fn try_claim(&self) -> bool {
        self.is_running()
            && self
                .busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
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

    /// The operator-facing projection of this session, for the list pane.
    pub fn row(&self) -> SessionRow {
        let cold = lock(&self.cold);
        SessionRow {
            id: self.meta.id.clone(),
            label: self.meta.label.clone(),
            provider: self.meta.provider,
            state: self.state(),
            cwd: self.meta.cwd.clone(),
            session_id: cold.session_id.clone(),
            started_at: self.meta.started_at,
            last_output_at: self.last_output_at(),
            last_error: cold.last_error.clone(),
            busy: self.is_busy(),
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
    /// The pty ends are dropped here too. They are two file descriptors per
    /// session and nothing reads them once the child is gone, but they used to
    /// be held until an operator pressed the key that forgot the session — so a
    /// worker serving a long fan-out leaked a pair per finished task until it
    /// could not open any more.
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

    /// Write raw bytes to the pty.
    ///
    /// Only this session's `io` lock is held, and deliberately so. `write_all`
    /// on a pty master **blocks** when the child is not draining its input — mid
    /// tool call, stopped on a modal, or simply slow — and the kernel's tty
    /// buffer is only a few kilobytes, which an injected prompt can exceed on
    /// its own. Under the old single lock that parked every reader thread, every
    /// render frame, and every other session behind one wedged child.
    pub(super) fn write(&self, bytes: &[u8]) -> Result<(), String> {
        if !self.is_running() {
            return Err(format!("{} has exited", self.meta.id));
        }
        let mut io = lock(&self.io);
        let Some(io) = io.as_mut() else {
            return Err(format!("{} has exited", self.meta.id));
        };
        use std::io::Write as _;
        io.writer
            .write_all(bytes)
            .and_then(|()| io.writer.flush())
            .map_err(|err| format!("{}: {err}", self.meta.id))
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
