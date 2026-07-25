//! Data types for the worker PTY manager.
#[allow(unused_imports)]
use super::*;
/// A clock in epoch ms (injectable for tests).
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;
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
    pub(super) sessions: Mutex<Vec<PtySession>>,
    pub(super) next_id: AtomicU64,
    pub(super) now: NowFn,
}
