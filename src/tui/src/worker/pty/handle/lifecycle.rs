//! Birth and death of a [`SessionHandle`] — construction, killing, and reaping.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use portable_pty::{Child, MasterPty};

use super::super::sync::lock;
use super::super::types::SessionControl;
use super::types::{
    AttentionState, ColdFields, SessionIo, SessionMeta, NO_EXIT_CODE, STATE_EXITED, STATE_RUNNING,
};
use super::SessionHandle;

impl SessionHandle {
    /// Build a handle for a child that has just been spawned.
    ///
    /// `writes` is the sending half of the queue the session's writer thread
    /// drains, and `queued_bytes` the budget they share; see
    /// [`SessionIo::writes`].
    #[allow(clippy::too_many_arguments)]
    pub(in super::super) fn new(
        meta: SessionMeta,
        label: String,
        name: Option<String>,
        session_id: Option<String>,
        control: SessionControl,
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
        //
        // Origin, not control, answers this: what matters is whether a turn was
        // dispatched into this session at birth, and only the path that created
        // it knows that.
        let busy = !meta.origin.is_user();
        SessionHandle {
            meta,
            state: AtomicU8::new(STATE_RUNNING),
            exit_code: AtomicI64::new(NO_EXIT_CODE),
            last_output_at: AtomicI64::new(started_at),
            generation: AtomicU64::new(0),
            busy: AtomicBool::new(busy),
            operator_held: AtomicBool::new(control == SessionControl::User),
            queued_bytes,
            cold: Mutex::new(ColdFields {
                label,
                session_id,
                name,
                thread_name: None,
                last_error: None,
            }),
            attention: Mutex::new(AttentionState {
                cue: None,
                seen_bells: 0,
                generation: 0,
                pending_completion_bells: 0,
                completion_deadline: None,
                checked_at: started_at,
            }),
            screen: Mutex::new(screen),
            io: Mutex::new(Some(SessionIo { master, writes })),
            child: Mutex::new(Some(child)),
        }
    }

    /// Ask the child to die. Does not wait for it.
    pub(in super::super) fn kill(&self) {
        if let Some(child) = lock(&self.child).as_mut() {
            let _ = child.kill();
        }
    }

    /// Note that the session has ended, without waiting on the child.
    ///
    /// What `close` does: the reader thread sees the pty close a moment later
    /// and [`reap`](SessionHandle::reap) settles the real status.
    pub(in super::super) fn mark_closed(&self) {
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
    pub(in super::super) fn reap(&self, now: i64) {
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
}
