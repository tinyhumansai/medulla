//! Data types for the `activity` module.
#[allow(unused_imports)]
use super::*;
/// One thing a worker did, as the hub observed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerActivity {
    /// The roster id of the worker this belongs to, when the task's dispatch
    /// was seen. Empty when a frame arrives for a task this hub never sent —
    /// which is possible, because the backend broadcasts to every harness.
    pub agent_id: String,
    /// The wire task id.
    pub task_id: String,
    /// The frame kind: `ack`, `status`, `reply`, `error`.
    pub kind: String,
    /// The frame's text, as it arrived.
    pub content: String,
    /// Epoch ms when the hub saw it.
    pub at: i64,
}
/// A bounded, shared record of worker activity.
///
/// Cheap to clone; every clone reads and writes the same ring.
#[derive(Clone, Default)]
pub struct ActivityLog {
    pub(super) entries: Arc<Mutex<VecDeque<WorkerActivity>>>,
    /// Which worker each task was dispatched to. Written at dispatch, read when
    /// its frames come back.
    pub(super) attribution: Arc<Mutex<VecDeque<(String, String)>>>,
}
