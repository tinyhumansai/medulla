//! Data types for the `activity` module.
#[allow(unused_imports)]
use super::*;
/// One thing a worker did, as the hub observed it.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerActivity {
    /// The roster id of the worker this belongs to, when the task's dispatch
    /// was seen. Empty when a frame arrives for a task this hub never sent —
    /// which is possible, because the backend broadcasts to every harness.
    pub agent_id: String,
    /// The operator-facing task id used to render and cancel this activity.
    pub task_id: String,
    /// The frame kind: `ack`, `status`, `reply`, `error`.
    pub kind: String,
    /// The frame's text, as it arrived.
    pub content: String,
    /// Epoch ms when the hub saw it.
    pub at: i64,
    /// What the worker was working on as of this frame, when it reported one.
    ///
    /// A worker that predates the work surface, or one running a harness that
    /// reports nothing structured, leaves this absent — which the UI renders as
    /// no work panel rather than an empty one.
    pub work: Option<Box<crate::harness_work::WorkSnapshot>>,
}

/// Correlates an inbound wire id with the task row the operator controls.
#[derive(Clone)]
pub(super) struct ActivityAttribution {
    /// Task id echoed by the worker in its frames.
    pub(super) observed_task_id: String,
    /// Task id displayed in the UI and accepted by its abort action.
    pub(super) activity_task_id: String,
    /// Roster id of the worker running the task.
    pub(super) agent_id: String,
}

/// A bounded, shared record of worker activity.
///
/// Cheap to clone; every clone reads and writes the same ring.
#[derive(Clone, Default)]
pub struct ActivityLog {
    pub(super) entries: Arc<Mutex<VecDeque<WorkerActivity>>>,
    /// How inbound wire ids map to operator-visible task ids and workers.
    /// Written at dispatch and read when frames come back.
    pub(super) attribution: Arc<Mutex<VecDeque<ActivityAttribution>>>,
}
