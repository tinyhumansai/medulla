//! Data model for headless provider runs: the callback aliases, the cooperative
//! [`Abort`] handle, and the input/output records ([`RunTaskOptions`],
//! [`RunTaskResult`]) plus the injectable executor alias [`RunTaskFn`]. The
//! detection and execution logic lives in the sibling `detect`/`execute`
//! modules.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{mpsc, Notify};

use crate::tinyplace::HarnessProvider;
use std::collections::HashMap;

use super::super::mappers::HarnessSemanticEvent;

/// A per-event status callback (drives daemon status frames).
pub type OnEvent = Box<dyn FnMut(&HarnessSemanticEvent) + Send>;
/// A one-shot registration of a child-stdin sender for `input` forwarding.
pub type OnStdin = Box<dyn FnOnce(mpsc::UnboundedSender<String>) + Send>;

/// A PATH-lookup predicate (injectable for tests).
pub type ExistsOnPath = Box<dyn Fn(&str) -> bool + Send + Sync>;

/// A cooperative abort handle shared between the daemon and a running task.
/// Aborting sets the flag and wakes any waiter; a task selects on
/// [`Abort::cancelled`] to terminate its child (SIGTERM).
#[derive(Clone, Default)]
pub struct Abort {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl Abort {
    /// Create a fresh, un-signalled abort handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Signal cancellation.
    pub fn abort(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Whether cancellation has been signalled.
    pub fn is_aborted(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Resolve once cancellation is signalled (immediately if already aborted).
    pub async fn cancelled(&self) {
        loop {
            if self.is_aborted() {
                return;
            }
            let notified = self.notify.notified();
            if self.is_aborted() {
                return;
            }
            notified.await;
        }
    }
}

/// Inputs for one headless run.
pub struct RunTaskOptions {
    /// The coding-agent CLI to spawn.
    pub provider: HarnessProvider,
    /// The task text handed to the provider.
    pub prompt: String,
    /// Working directory for the child process.
    pub cwd: String,
    /// The full environment the child runs with (the parent env is cleared).
    pub env: HashMap<String, String>,
    /// Idle watchdog budget in ms; each observed event pushes the deadline out.
    pub timeout_ms: u64,
    /// Optional model override.
    pub model: Option<String>,
    /// Optional agent selector (opencode).
    pub agent: Option<String>,
    /// Extra provider argv appended to the built base args.
    pub extra_args: Vec<String>,
    /// Whether to pass the provider's skip-permissions flag.
    pub skip_permissions: bool,
    /// The cooperative abort handle.
    pub abort: Abort,
    /// Fired for each parsed semantic event — drives periodic status frames.
    pub on_event: Option<OnEvent>,
    /// Register a stdin channel for `input`-frame forwarding into the child.
    pub on_stdin: Option<OnStdin>,
}

/// The outcome of a headless run.
#[derive(Debug, Clone)]
pub struct RunTaskResult {
    /// The provider that produced this result.
    pub provider: HarnessProvider,
    /// The agent's final answer (concatenated assistant text, or a fallback).
    pub reply: String,
    /// Count of semantic events observed.
    pub events: usize,
    /// Latest token usage the child reported on its stream, if any.
    pub usage: Option<crate::tinyplace::TokenUsage>,
}

/// The injectable executor signature (the daemon runtime defaults to
/// [`run_provider_task`](super::run_provider_task); tests supply a fake).
pub type RunTaskFn = Arc<
    dyn Fn(RunTaskOptions) -> Pin<Box<dyn Future<Output = Result<RunTaskResult, String>> + Send>>
        + Send
        + Sync,
>;

/// The plain-data slice of a run (no callbacks), so it stays `Send + Sync` and a
/// borrow of it can live across the child-process awaits.
///
/// Raised to `pub(super)` (from a private struct) so the sibling `execute`
/// module can construct it and read its fields.
pub(super) struct RunSpec {
    pub(super) provider: HarnessProvider,
    pub(super) prompt: String,
    pub(super) cwd: String,
    pub(super) env: HashMap<String, String>,
    pub(super) timeout_ms: u64,
    pub(super) model: Option<String>,
    pub(super) agent: Option<String>,
    pub(super) extra_args: Vec<String>,
    pub(super) skip_permissions: bool,
    pub(super) abort: Abort,
}
