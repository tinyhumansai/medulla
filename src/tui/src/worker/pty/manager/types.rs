//! Data types for the worker PTY manager.
#[allow(unused_imports)]
use super::*;
/// A clock in epoch ms (injectable for tests).
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;
/// Where a harness's own clipboard writes (OSC 52) are sent on to.
///
/// Injectable for the same reason the clock is: the real one writes escape
/// sequences to this process's terminal and shells out to `tmux`/`pbcopy`, none
/// of which a test can observe or should trigger.
pub type ClipboardSink = Arc<dyn Fn(&str) + Send + Sync>;
/// Owns the live harness sessions the worker TUI renders.
///
/// Cheap to clone (an `Arc`), so the daemon's inbound-frame path and the render
/// loop share one.
#[derive(Clone)]
pub struct PtyManager {
    pub(super) inner: Arc<Inner>,
}
pub(super) struct Inner {
    /// Sessions in open order, so the list does not reshuffle under the cursor.
    ///
    /// An `RwLock` over `Arc` handles rather than a `Mutex` over the session
    /// bodies, which is the point of the split. The registry is read-mostly — it
    /// changes only when a session is opened or forgotten — while lookups happen
    /// on every frame, every reader wakeup and every executor poll. Readers no
    /// longer exclude one another and, more importantly, **nobody holds this
    /// lock while doing work**: every accessor clones the one `Arc` it wants and
    /// releases the registry before touching the session at all.
    pub(super) sessions: RwLock<Vec<Arc<SessionHandle>>>,
    pub(super) next_id: AtomicU64,
    pub(super) now: NowFn,
    /// Feeds the single clipboard worker thread. See
    /// [`PtyManager::forward_clipboard`](super::PtyManager::forward_clipboard).
    pub(super) clipboard: Arc<ClipboardQueue>,
}

/// A single-slot, coalescing handoff to the clipboard worker thread.
///
/// Not a channel: clipboard semantics are last-write-wins, so queueing every
/// copy an unbounded `mpsc` would is both unnecessary and unsafe — a harness
/// that emits OSC 52 in a tight loop (buggy or hostile) could otherwise grow
/// the backlog without limit while the worker sits blocked in `tmux`/`pbcopy`
/// for seconds at a time. Holding only the newest pending copy keeps memory
/// bounded and means the worker, once it catches up, always writes what the
/// operator most recently copied rather than working through a queue of
/// stale values.
pub(super) struct ClipboardQueue {
    pending: std::sync::Mutex<Option<String>>,
    signal: std::sync::Condvar,
    closed: std::sync::atomic::AtomicBool,
}

impl ClipboardQueue {
    pub(super) fn new() -> Self {
        ClipboardQueue {
            pending: std::sync::Mutex::new(None),
            signal: std::sync::Condvar::new(),
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Replace whatever copy is waiting with `text` and wake the worker.
    pub(super) fn set(&self, text: String) {
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(text);
        self.signal.notify_one();
    }

    /// Block until a copy is pending or the queue is closed, then take it.
    ///
    /// `None` means closed: the worker thread's loop ends and it exits.
    pub(super) fn take_blocking(&self) -> Option<String> {
        let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(text) = guard.take() {
                return Some(text);
            }
            if self.closed.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            guard = self
                .signal
                .wait(guard)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Wake the worker thread so it observes `closed` and exits.
    pub(super) fn close(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::Release);
        self.signal.notify_one();
    }
}
