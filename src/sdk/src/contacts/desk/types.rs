//! Data types for the `desk` module.
#[allow(unused_imports)]
use super::*;
/// The outcome of the most recent poll of the relay.
///
/// Without this, a relay call that fails every tick is indistinguishable from a
/// queue that is simply empty — the operator watches an empty list and cannot
/// tell whether nobody has asked or nothing is being asked *of*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollHealth {
    /// No poll has completed yet.
    Pending,
    /// The relay answered. `at` is epoch ms.
    Ok {
        /// When it answered.
        at: i64,
        /// How many requests it reported, settled or not.
        seen: usize,
    },
    /// The relay could not be reached.
    Failed {
        /// When the attempt was made.
        at: i64,
        /// Why it failed.
        error: String,
    },
}
/// Everything the Sessions screen needs to manage incoming contact requests.
///
/// Cheap to clone; the clones share one book.
#[derive(Clone)]
pub struct ContactDesk {
    pub(super) book: ContactBook,
    pub(super) relay: Arc<dyn ContactRelay>,
    pub(super) now: NowFn,
    pub(super) health: Arc<Mutex<PollHealth>>,
    /// Where contact activity is narrated. A worker that appears to receive
    /// nothing and a worker that is never asked for anything look identical
    /// otherwise, and only one of them is a problem with the worker.
    ///
    /// Shared across clones like everything else here. It was per-instance, and
    /// the consequence was subtle: the poll runs on whichever handle called
    /// `spawn_poll`, so attaching a sink to a *different* handle narrated
    /// nothing — and the natural fix, polling the handle you attached to,
    /// silently gave you two polls of the same relay.
    pub(super) log: Arc<Mutex<Option<crate::logging::LineSink>>>,
}
