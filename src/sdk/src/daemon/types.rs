//! Daemon data model: the callback type aliases, non-callback [`DaemonConfig`],
//! the shared per-runtime [`Inner`] state, and the cheaply-clonable
//! [`DaemonRuntime`] handle.
//!
//! Only the data lives here; the runtime's behaviour is implemented beside the
//! logic that uses it in [`super::runtime`] (construction/dispatch/reply) and
//! [`super::task_loop`] (frame + task orchestration).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{mpsc, Mutex as TokioMutex, Notify, Semaphore};

use crate::tinyplace::{AgentCapabilities, HarnessProvider};

use super::providers::{Abort, RunTaskFn};

/// Default throttle between forwarded status frames, in epoch ms.
pub(super) const DEFAULT_STATUS_THROTTLE_MS: i64 = 4_000;
/// Default cap on admitted-but-unfinished tasks before the daemon sheds load.
pub(super) const DEFAULT_MAX_PENDING: usize = 16;
/// Default timeout for the on-demand capability probe, in ms.
pub(super) const DEFAULT_CAPABILITY_TIMEOUT_MS: u64 = 60_000;

/// How many status-throttle windows of total silence pass before the task loop
/// emits an unconditional heartbeat frame.
///
/// The heartbeat exists because the requester's liveness watchdog is reset by
/// *frames*, not by the task still being alive: a task waiting for a harness
/// slot, or running one long tool call, produces no semantic events at all and
/// looks identical to a crashed worker. Expressed as a multiple of
/// [`DaemonConfig::status_throttle_ms`] so it scales with how chatty the
/// operator asked this worker to be — and so tests can shrink it without a new
/// config knob. At the default throttle this is one frame every 32s, well
/// inside the hub's 240s no-progress window.
pub(super) const HEARTBEAT_THROTTLE_WINDOWS: i64 = 8;

/// Upper bound on the derived heartbeat period, in ms.
///
/// A very slack `status_throttle_ms` must not push the heartbeat past the
/// requester's no-progress window and reintroduce the very timeout it prevents.
pub(super) const MAX_HEARTBEAT_MS: i64 = 60_000;

/// The leading text of every load-shed rejection this daemon sends.
///
/// Shared with the requesting side (`crate::hub`) so backpressure can be
/// recognised as backpressure — a retryable "come back later", not a terminal
/// task failure — instead of being matched on a duplicated string literal.
pub const CAPACITY_REJECTION_PREFIX: &str = "daemon at capacity";

/// A lock-serialized encrypted send: `(to, body) -> ()`. Errors are handled by
/// the transport (logged), so the runtime never observes a send failure.
pub type SendFn =
    Arc<dyn Fn(String, String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A clock, in epoch ms (injectable for tests).
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// A line sink for daemon diagnostics. See [`crate::logging::LineSink`].
pub type LogFn = crate::logging::LineSink;

/// Non-callback daemon configuration.
#[derive(Clone)]
pub struct DaemonConfig {
    /// Providers this daemon will accept tasks for.
    pub providers: Vec<HarnessProvider>,
    /// Provider used when a task/plain-text message names none.
    pub default_provider: HarnessProvider,
    /// Absolute working directory tasks run in.
    pub workspace: String,
    /// Workspace roots the operator explicitly allows this worker to advertise.
    ///
    /// The primary [`workspace`](Self::workspace) is always included even when
    /// this list is empty. These paths are capability metadata for routing; a
    /// peer still cannot select an arbitrary working directory in a task frame.
    pub accessible_dirs: Vec<String>,
    /// Environment passed to spawned provider processes.
    pub env: HashMap<String, String>,
    /// Per-task execution timeout, in ms.
    pub task_timeout_ms: u64,
    /// Optional override for the capability-probe timeout, in ms.
    pub capability_timeout_ms: Option<u64>,
    /// Maximum concurrent task executions.
    pub concurrency: usize,
    /// Minimum spacing between forwarded status frames, in ms.
    pub status_throttle_ms: i64,
    /// Maximum admitted-but-unfinished tasks before shedding load.
    pub max_pending: usize,
    /// Default model hint passed to providers.
    pub model: Option<String>,
    /// opencode agent selector, when applicable.
    pub agent: Option<String>,
    /// Extra CLI args forwarded to every provider invocation.
    pub extra_args: Vec<String>,
    /// Whether to pass the provider's skip-permissions flag.
    pub skip_permissions: bool,
    /// Optional custom OpenAI-compatible router. When set, every task this daemon
    /// runs has the provider's routed endpoint (and, if configured, its API key
    /// resolved from [`env`](Self::env) by name) layered into the spawn
    /// environment. `None` means routing is off.
    pub router: Option<crate::config::RouterConfig>,
    /// Operator-declared per-provider token budgets from the `[budget]` config.
    /// When set, the capability probe advertises `source: configured` budgets for
    /// matching providers instead of estimates. `None` means estimates only.
    pub budget: Option<crate::config::BudgetConfig>,
}

/// Bookkeeping for a single in-flight task keyed by `sender + taskId`.
pub(super) struct RunningTask {
    /// Provider selected for this task.
    pub(super) provider: HarnessProvider,
    /// Correlation id that scopes follow-up `input` frames to this dispatch.
    pub(super) correlation_id: Option<String>,
    /// Live stdin sender once the child accepts input.
    pub(super) stdin: Option<mpsc::UnboundedSender<String>>,
    /// Input buffered before the child's stdin became available.
    pub(super) pending_input: Vec<String>,
    /// The worker-local session this task is running in, once the executor has
    /// opened one. What a screen subscription naming this task resolves to.
    pub(super) session_id: Option<String>,
    /// Stops this task. Held per-task rather than only in the global controller
    /// map so an `abort` frame can cancel exactly the task it names.
    pub(super) abort: super::providers::Abort,
}

/// One admitted-but-unfinished task, released on drop.
///
/// The admission count is the daemon's load-shed gate: once it reaches
/// [`DaemonConfig::max_pending`] every further task is rejected. It therefore
/// has to be released on *every* exit from a task handler, including an unwind
/// — the handler runs under a bare `tokio::spawn` that is never joined, so a
/// panic inside it is swallowed silently, and a hand-rolled `fetch_sub` on the
/// straight-line path would leak the slot permanently. Sixteen such unwinds pin
/// the daemon at capacity until it is restarted.
///
/// The same reasoning covers the two records whose lifetime matches the
/// admission: the `running` entry (a stranded one makes its task id refuse
/// every later dispatch as "already running") and the controller registration.
/// Both are attached to the guard so they cannot be released separately.
pub(super) struct AdmissionGuard {
    /// Runtime state the guard releases into.
    inner: Arc<Inner>,
    /// `running` map key to remove, once the task has been registered.
    key: Option<String>,
    /// Controller id to unregister, once one has been allocated.
    controller: Option<u64>,
}

impl AdmissionGuard {
    /// Claim admission, or `None` when the daemon is already at capacity.
    ///
    /// Check and increment are one `compare_exchange` so two concurrently
    /// spawned handlers cannot both observe the last free slot and overshoot
    /// `max_pending`.
    pub(super) fn claim(inner: &Arc<Inner>) -> Option<Self> {
        let max = inner.config.max_pending;
        let mut current = inner.admitted.load(Ordering::SeqCst);
        loop {
            if current >= max {
                return None;
            }
            match inner.admitted.compare_exchange_weak(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    return Some(AdmissionGuard {
                        inner: inner.clone(),
                        key: None,
                        controller: None,
                    })
                }
                Err(observed) => current = observed,
            }
        }
    }

    /// Tie the `running` record under `key` to this admission.
    pub(super) fn attach_task(&mut self, key: String) {
        self.key = Some(key);
    }

    /// Tie the controller registration `id` to this admission.
    pub(super) fn attach_controller(&mut self, id: u64) {
        self.controller = Some(id);
    }
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        // A poisoned map must still release the count: recovering the guard
        // (`into_inner`) is strictly better than panicking a second time and
        // wedging the load-shed gate for the rest of the process's life.
        if let Some(key) = self.key.take() {
            self.inner
                .running
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key);
        }
        if let Some(id) = self.controller.take() {
            self.inner
                .controllers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
        }
        self.inner.admitted.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Shared, `Arc`-wrapped runtime state behind [`DaemonRuntime`].
pub(super) struct Inner {
    /// Static configuration.
    pub(super) config: DaemonConfig,
    /// The task executor.
    pub(super) run_task: RunTaskFn,
    /// Lock-serialized encrypted send.
    pub(super) send: SendFn,
    /// Injectable clock.
    pub(super) now: NowFn,
    /// Optional diagnostics sink.
    pub(super) log: Option<LogFn>,
    /// Active tasks keyed by `sender + taskId`.
    pub(super) running: StdMutex<HashMap<String, RunningTask>>,
    /// Abort handles for every in-flight run, keyed by controller id.
    pub(super) controllers: StdMutex<HashMap<u64, Abort>>,
    /// Monotonic source of controller ids.
    pub(super) next_controller_id: AtomicU64,
    /// Count of admitted-but-unfinished tasks (load-shed gate).
    pub(super) admitted: AtomicUsize,
    /// Concurrency budget.
    pub(super) slots: Semaphore,
    /// Count of dispatched messages not yet fully settled.
    pub(super) inflight_count: AtomicUsize,
    /// Notified when `inflight_count` reaches zero (drives [`DaemonRuntime::idle`]).
    pub(super) inflight_idle: Notify,
    /// Cached capability probe result.
    pub(super) capabilities: TokioMutex<Option<AgentCapabilities>>,
    /// Workspace roots currently approved for capability advertisement.
    ///
    /// Kept outside the immutable config so the daemon TUI can change the
    /// allowlist without restarting the worker.
    pub(super) accessible_dirs: StdMutex<Vec<String>>,
}

/// The provider-agnostic daemon task state machine. Cheap to clone (an `Arc`),
/// so it can be handed to spawned dispatches.
#[derive(Clone)]
pub struct DaemonRuntime {
    /// Shared state; see [`Inner`].
    pub(super) inner: Arc<Inner>,
}

/// Optional payloads a task frame can carry beyond its text.
///
/// Grouped into one value rather than threaded as parameters because the list
/// grows with the protocol: token usage was the first, the child harness's work
/// snapshot the second, and every addition would otherwise widen four call
/// signatures.
#[derive(Debug, Clone, Default)]
pub(super) struct FrameAttachments {
    /// Token counts the child harness reported, on reply frames.
    pub(super) usage: Option<crate::tinyplace::TokenUsage>,
    /// What the child harness is working on as of this frame.
    pub(super) work: Option<crate::harness_work::WorkSnapshot>,
}
