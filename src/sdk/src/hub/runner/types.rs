//! Data types for hub task dispatch and reply correlation.
#[allow(unused_imports)]
use super::*;
/// A registered dispatch awaiting its terminal frame.
pub(super) struct Waiter {
    /// Resolved with the terminal outcome (`reply`) or the worker's error text.
    pub(super) reply: oneshot::Sender<Result<TaskOutcome, String>>,
    /// Optional progress sink fed by `status` frames while the task runs.
    pub(super) status: Option<mpsc::UnboundedSender<String>>,
    /// Notified on ANY inbound frame for this dispatch — the "peer is alive"
    /// signal the runner's ack window waits on.
    pub(super) activity: Arc<Notify>,
}
/// Shared registry of in-flight dispatches, keyed by `correlationId`.
pub(super) type Waiters = Arc<Mutex<HashMap<String, Waiter>>>;
/// In-flight lightweight system probes, keyed by correlation id.
pub(super) type SystemInfoWaiters =
    Arc<Mutex<HashMap<String, oneshot::Sender<Result<WorkerSystemInfo, String>>>>>;
/// Shared registry of abort signals, keyed by the orchestrator-facing task id
/// (`medulla:task_abort.taskId`). One entry per in-flight [`TaskRunner::run`],
/// registered for the whole call; notifying it makes that call stop the worker,
/// reap its correlation entry, and return [`RunError::Aborted`].
///
/// A `std::sync::Mutex` (not tokio's), because the [`AbortGuard`] that removes an
/// entry runs in `Drop`, which cannot await — and the lock is only ever held for
/// a synchronous get/insert/remove, never across a suspension point.
pub(super) type Aborts = Arc<std::sync::Mutex<HashMap<String, Arc<Notify>>>>;
/// Removes a `run` call's abort signal from the [`Aborts`] registry when the call
/// returns (by any path), so a settled dispatch leaves nothing behind for a later
/// `task_abort` to match. Removes only its OWN signal (identity-compared), so a
/// concurrent re-dispatch that reused the same id and overwrote the slot is left
/// intact.
pub(super) struct AbortGuard {
    pub(super) aborts: Aborts,
    pub(super) key: String,
    pub(super) signal: Arc<Notify>,
}
/// Sends tasks to remote tiny.place workers and correlates their replies.
///
/// Holds a shared [`Relay`] and a background pump that drains the encrypted
/// inbox and fans decoded frames to per-dispatch waiters. Wrap in `Arc` to share
/// across dispatches; dropping it aborts the pump.
pub struct TaskRunner {
    pub(super) relay: Arc<dyn Relay>,
    pub(super) waiters: Waiters,
    /// System-information probes waiting for a worker response.
    pub(super) system_info_waiters: SystemInfoWaiters,
    /// Abort signals for in-flight dispatches, keyed by orchestrator-facing task
    /// id; [`abort_task`](Self::abort_task) notifies one to cancel its dispatch.
    pub(super) aborts: Aborts,
    pub(super) counter: AtomicU64,
    /// How long a dispatch waits for the first sign of life before re-handshaking.
    pub(super) ack_window: Duration,
    /// The no-progress window applied once the peer is alive (see [`IDLE_WINDOW`]).
    pub(super) idle_window: Duration,
    pub(super) pump: tokio::task::JoinHandle<()>,
}
