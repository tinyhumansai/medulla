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
    /// Where a session's forwarded clipboard writes go. See
    /// [`clipboard`](super::clipboard).
    pub(super) clipboard: ClipboardSink,
}
