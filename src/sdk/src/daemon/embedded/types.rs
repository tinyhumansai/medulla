//! Data types for the embedded daemon: its start-up options, its live
//! observation snapshot, and the handle a host keeps it alive by.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::{BudgetConfig, RouterConfig};
use crate::tinyplace::HarnessProvider;

use super::super::types::{DaemonRuntime, LogFn};

/// Default inbox poll interval. Tighter than the networked daemon's because the
/// local bus is an in-memory queue: there is no relay to be polite to, and this
/// interval is the floor on how quickly a locally-dispatched task starts.
pub const DEFAULT_LOCAL_POLL: Duration = Duration::from_millis(150);
/// Default concurrent task executions for an embedded host.
pub const DEFAULT_CONCURRENCY: usize = 2;
/// Default per-task execution timeout, in ms.
pub const DEFAULT_TASK_TIMEOUT_MS: u64 = 600_000;

/// How to run this device as a task host.
///
/// Every field has a working default, so a host that wants "this machine, this
/// directory, whatever agent CLI is installed" can take [`Default`] and set only
/// [`workspace`](Self::workspace).
#[derive(Clone)]
pub struct EmbeddedDaemonOptions {
    /// Absolute working directory tasks run in. Empty resolves to the process's
    /// current directory at start.
    pub workspace: String,
    /// Providers to serve. `None` detects whatever coding-agent CLIs are on
    /// `PATH`; an explicit list is still filtered by what is actually installed.
    pub providers: Option<Vec<HarnessProvider>>,
    /// Provider used when a task frame names none. `None` takes the first
    /// detected provider.
    pub default_provider: Option<HarnessProvider>,
    /// Maximum concurrent task executions.
    pub concurrency: usize,
    /// Per-task execution timeout, in ms.
    pub task_timeout_ms: u64,
    /// How often the inbox is drained.
    pub poll: Duration,
    /// Default model hint passed to providers.
    pub model: Option<String>,
    /// opencode agent selector, when applicable.
    pub agent: Option<String>,
    /// Whether to pass the provider's skip-permissions flag. Defaults to `true`:
    /// an embedded host runs tasks nobody is watching, and a harness that stops
    /// on a permission prompt has hung until its timeout.
    pub skip_permissions: bool,
    /// Environment spawned providers inherit. Empty reads the process env.
    pub env: HashMap<String, String>,
    /// Optional custom OpenAI-compatible router layered into every spawn.
    pub router: Option<RouterConfig>,
    /// Operator-declared per-provider token budgets.
    pub budget: Option<BudgetConfig>,
    /// Where diagnostics go. `None` discards them, which is what a TUI wants
    /// until it supplies a sink it can render.
    pub log: Option<LogFn>,
}

impl Default for EmbeddedDaemonOptions {
    fn default() -> Self {
        EmbeddedDaemonOptions {
            workspace: String::new(),
            providers: None,
            default_provider: None,
            concurrency: DEFAULT_CONCURRENCY,
            task_timeout_ms: DEFAULT_TASK_TIMEOUT_MS,
            poll: DEFAULT_LOCAL_POLL,
            model: None,
            agent: None,
            skip_permissions: true,
            env: HashMap::new(),
            router: None,
            budget: None,
            log: None,
        }
    }
}

/// What the embedded host has been doing, for a UI to render.
///
/// Counters rather than a history: the host is one lane in a fleet view, and the
/// transcript of any individual task already belongs to the orchestrator that
/// dispatched it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmbeddedDaemonStats {
    /// Frames received from peers (task, input, abort, probes).
    pub received: u64,
    /// Tasks admitted for execution.
    pub tasks_started: u64,
    /// Tasks that produced a terminal `reply`.
    pub tasks_completed: u64,
    /// Tasks that produced a terminal `error`.
    pub tasks_failed: u64,
    /// Capability and system-info probes answered.
    pub probes_answered: u64,
    /// The most recent status line the host emitted, if any.
    pub last_status: Option<String>,
    /// The address of the peer most recently spoken to.
    pub last_peer: Option<String>,
}

impl EmbeddedDaemonStats {
    /// Tasks admitted but not yet terminal.
    ///
    /// Derived rather than tracked so it can never disagree with the counters it
    /// is computed from. Saturating because a terminal frame for a task admitted
    /// before this process started would otherwise underflow.
    pub fn tasks_running(&self) -> u64 {
        self.tasks_started
            .saturating_sub(self.tasks_completed)
            .saturating_sub(self.tasks_failed)
    }
}

/// The shared observation slot a host writes and a UI reads.
pub type SharedStats = Arc<Mutex<EmbeddedDaemonStats>>;

/// A running embedded host.
///
/// Dropping it stops the drain loop and cancels in-flight tasks, so a host that
/// outlives its owner is impossible. Clones are not provided deliberately: two
/// owners of one drain loop would each believe they control its lifetime.
pub struct EmbeddedDaemon {
    /// The address this host is reachable at on its bridge.
    pub(super) address: String,
    /// Absolute working directory tasks run in.
    pub(super) workspace: String,
    /// Providers actually detected and being served.
    pub(super) providers: Vec<HarnessProvider>,
    /// The provider used when a task names none.
    pub(super) default_provider: HarnessProvider,
    /// The task state machine, exposed so a host can query capabilities.
    pub(super) runtime: DaemonRuntime,
    /// Live counters for a UI.
    pub(super) stats: SharedStats,
    /// The drain loop, aborted on drop.
    pub(super) drain: tokio::task::JoinHandle<()>,
}

/// Hand-written because the runtime, executor, and join handle have no useful
/// representation — what identifies a host is where it is and what it serves.
impl std::fmt::Debug for EmbeddedDaemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddedDaemon")
            .field("address", &self.address)
            .field("workspace", &self.workspace)
            .field("providers", &self.providers)
            .field("default_provider", &self.default_provider)
            .finish_non_exhaustive()
    }
}

impl Drop for EmbeddedDaemon {
    fn drop(&mut self) {
        self.drain.abort();
        self.runtime.shutdown();
    }
}
