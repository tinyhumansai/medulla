//! Data types for hub task dispatch and reply correlation.
#[allow(unused_imports)]
use super::*;
/// A registered dispatch awaiting its terminal frame.
pub(super) struct Waiter {
    /// The worker address this dispatch was sent to — the only sender whose
    /// frames may settle it. See [`Probe::from`].
    pub(super) from: String,
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
/// A probe waiting on one worker's answer.
///
/// The address is carried, not just the channel, because a correlation id is not
/// a secret: probe ids are predictable counters, and the inbox is shared by every
/// peer this identity has a contact edge with. Binding the waiter to the address
/// the probe was sent to is what stops a peer that guesses an id from answering
/// on another worker's behalf.
pub(super) struct Probe<T> {
    /// The worker address the probe was dispatched to.
    pub(super) from: String,
    /// Resolved with the worker's answer, or its error text.
    pub(super) tx: oneshot::Sender<Result<T, String>>,
}
/// In-flight lightweight system probes, keyed by correlation id.
pub(super) type SystemInfoWaiters = Arc<Mutex<HashMap<String, Probe<WorkerSystemInfo>>>>;
/// In-flight capability probes, keyed by correlation id. Each resolves with the
/// worker's self-reported [`AgentCapabilities`] (carrying its budgets/readiness)
/// or the worker's error text.
///
/// This uses a synchronous mutex because [`CapabilityProbeGuard`] must remove a
/// cancelled probe during `Drop`, which cannot await. The lock is never held
/// across a suspension point.
pub(super) type CapabilitiesWaiters =
    Arc<std::sync::Mutex<HashMap<String, Probe<AgentCapabilities>>>>;
/// Removes a capability waiter when its future completes or is cancelled.
pub(super) struct CapabilityProbeGuard {
    pub(super) waiters: CapabilitiesWaiters,
    pub(super) key: String,
}
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
/// Sends tasks over a bridge and correlates their replies.
///
/// Holds a shared [`Relay`] and a background pump that drains its inbox and fans
/// decoded frames to per-dispatch waiters. Wrap in `Arc` to share across
/// dispatches; dropping it aborts the pump.
pub struct TaskRunner {
    pub(super) relay: Arc<dyn Relay>,
    /// The worker screens this hub is watching, filled by the pump.
    pub(super) screens: crate::hub::ScreenStore,
    pub(super) waiters: Waiters,
    /// System-information probes waiting for a worker response.
    pub(super) system_info_waiters: SystemInfoWaiters,
    /// Capability probes waiting for a worker's `capabilities_result`.
    pub(super) capabilities_waiters: CapabilitiesWaiters,
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
