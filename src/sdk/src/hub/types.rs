//! Data types for the bridge-independent task sender: a dispatch request, its
//! terminal outcome, and the error a dispatch can fail with.

use serde_json::{Map, Value};

use crate::protocol::{HarnessProvider, TokenUsage};

/// A line sink for hub diagnostics.
///
/// The hub used to write these straight to stderr, which is fine for
/// `medulla hub` but corrupts the orchestrator TUI: ratatui owns the alternate
/// screen and only repaints the cells it manages, so anything else written to
/// the terminal lands on top and is never cleared. Injecting the sink lets a
/// caller that owns the screen route the lines somewhere it can render them.
pub type HubLog = crate::logging::LineSink;

/// Where roster changes are written so they survive a restart.
///
/// A callback rather than a config-file write inside the handle: the hub should
/// not have to know where an embedding host keeps its settings, and a test needs
/// to observe the roster without touching a disk.
pub type RosterSink = std::sync::Arc<dyn Fn(&[super::HubWorker]) + Send + Sync>;

/// The default sink: stderr, as before, for callers that own the terminal.
pub fn stderr_log() -> HubLog {
    crate::logging::stderr_sink()
}

/// A single task to dispatch to a local or remote worker.
#[derive(Debug, Clone)]
pub struct TaskRequest {
    /// Worker-facing task id (echoed on the frame; the worker returns it). This
    /// is the wire id, made unique per dispatch so a worker never dedupes two
    /// different pieces of work that happen to share a name.
    pub task_id: String,
    /// The orchestrator-facing task id the backend aborts by
    /// (`medulla:task_abort.taskId`). The runner registers this dispatch's abort
    /// signal under it, so a `task_abort` for this id cancels the dispatch.
    /// Distinct from [`task_id`](Self::task_id), which is the worker-facing wire id.
    pub abort_id: String,
    /// Correlates every frame in one cycle; `None` uses the literal `"cyc"`.
    pub cycle_id: Option<String>,
    /// The instruction/prompt the worker runs.
    pub instruction: String,
    /// The worker's bridge address: a local endpoint name or tiny.place id.
    pub worker_address: String,
    /// Optional harness hint (`claude`/`codex`/`opencode`).
    pub provider: Option<HarnessProvider>,
    /// Optional named custom harness preset exposed by the worker.
    pub custom_harness: Option<String>,
    /// Optional model hint (the worker maps it to `--model`/`-m`, else its
    /// configured default).
    pub model: Option<String>,
    /// Which slice of the workflow tools this dispatch's harness is served.
    ///
    /// `None` — every ordinary dispatch — means the full authoring surface.
    /// `Some("propose:<workflow-id>")` withholds graph writes and scopes review writes,
    /// which is what an evolution pass gets.
    ///
    /// Carried on the request rather than read from the ambient environment
    /// because it varies *per turn*: the same daemon dispatches authoring turns
    /// and review turns minutes apart, and a process-wide switch could only
    /// ever be right for one of them.
    ///
    /// A plain string rather than the typed `ToolMode` so this type does not
    /// depend on the `workflows` feature being compiled in.
    pub tool_mode: Option<String>,
    /// Optional installed-workflow id: run that saved graph instead of handing
    /// [`instruction`](Self::instruction) to a harness as a prompt.
    ///
    /// This is what lets one dispatch be a whole plan. The instruction becomes
    /// the workflow's trigger payload.
    pub workflow: Option<String>,
    /// Fingerprint of the exact workflow definition the caller selected.
    ///
    /// Carried to the worker so it can compare the record it is about to run,
    /// closing the gap between a capability probe and actual execution.
    pub workflow_fingerprint: Option<String>,
    /// Values for the selected workflow's declared inputs, keyed by name.
    ///
    /// Empty for ordinary harness tasks and workflows with no declared inputs.
    pub workflow_inputs: Map<String, Value>,
    /// Opt into session continuity: successive dispatches naming the same
    /// conversation resume one harness session.
    ///
    /// `None` — the default, and what every dispatch but the copilot's uses —
    /// keeps a task context-free, which is the invariant that lets two tasks
    /// run concurrently without seeing each other's work.
    ///
    /// Continuity is best-effort by design. A provider with no resume flag
    /// (`opencode`) runs every turn fresh, which loses context but never
    /// correctness, so a caller may always ask.
    pub conversation: Option<String>,
    /// How deep in a dispatch tree the harness running this task will sit.
    ///
    /// `0` for work an operator started. A task dispatched *by* a harness
    /// through the fleet tools carries its dispatcher's depth plus one, which is
    /// what stops a tree from fanning out forever with nobody watching.
    ///
    /// Carried on the request rather than read from the ambient environment for
    /// the reason [`tool_mode`](Self::tool_mode) is: one process dispatches at
    /// several depths, so a process-wide value could only ever be right for one
    /// of them. It is set by the control plane from the *grant* the dispatcher
    /// presented, never from anything that dispatcher said about itself.
    pub fleet_depth: u8,
}

/// The terminal result of a dispatched task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOutcome {
    /// The worker's final reply text.
    pub reply: String,
    /// Token usage the worker reported (zeros when it reported none).
    pub usage: TokenUsage,
    /// The provider that actually ran the task, when the worker reported it.
    pub harness: Option<HarnessProvider>,
}

/// Why a dispatch failed.
///
/// Non-exhaustive: this enum grows as the hub learns to tell more kinds of
/// refusal apart, and each new variant is a fact a caller wants rather than a
/// breaking change it should have to absorb. Match with a `_` arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunError {
    /// A liveness bound reaped the dispatch: either the peer never showed any
    /// sign of life within the ack window across every reset+resend attempt, or
    /// it acked and then went silent past the no-progress (idle) window. Not a
    /// task deadline — that belongs to the orchestrator, which aborts separately.
    Timeout,
    /// The orchestrator aborted the task (`medulla:task_abort`): the hub told the
    /// worker to stop and gave up waiting. Terminal and NOT retryable — the
    /// backend deliberately cancelled it, so re-running would undo its intent.
    Aborted,
    /// The worker returned an `error` frame (carrying its message).
    Worker(String),
    /// The worker shed load rather than failing: it was already holding its
    /// maximum admitted-but-unfinished tasks and refused this one
    /// (`daemon at capacity …; retry later`).
    ///
    /// Distinguished from [`Worker`](Self::Worker) because it says nothing about
    /// the *task*: nothing was attempted, and the same dispatch a moment later
    /// will very likely succeed. Retryable, so the orchestrator re-dispatches
    /// under its own attempt ceiling and backoff instead of turning a "come back
    /// later" into a permanently failed task.
    Busy(String),
    /// The worker refused because a person is working in that workspace
    /// (`harness held by operator …`).
    ///
    /// Like [`Busy`](Self::Busy) it says nothing about the *task* — nothing was
    /// attempted — but it does not clear on the same timescale: a saturated
    /// daemon frees up in seconds, a workspace frees up when a person is done
    /// with it. Retryable so the orchestrator's own attempt ceiling and backoff
    /// apply, and reported with a distinct `reason` on the wire so it can prefer
    /// another host rather than waiting on this one.
    Held(String),
    /// The send itself failed, or the waiter was dropped (transport-shaped).
    Transport(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Timeout => write!(f, "bridge task timed out"),
            RunError::Aborted => write!(f, "task aborted by orchestrator"),
            RunError::Worker(m) => write!(f, "worker error: {m}"),
            RunError::Busy(m) => write!(f, "worker busy: {m}"),
            RunError::Held(m) => write!(f, "harness held by operator: {m}"),
            RunError::Transport(m) => write!(f, "transport error: {m}"),
        }
    }
}

impl std::error::Error for RunError {}
