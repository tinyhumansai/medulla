//! The data model for stored workflows and their runs.
//!
//! A *workflow* is a [`tinyflows::model::WorkflowGraph`] — the engine's own
//! portable JSON shape — plus the bookkeeping this host needs to find it, list
//! it, and say where it came from. The graph itself is deliberately not
//! re-modelled here: it is the contract shared with the engine and with the
//! sibling hosts that embed it, and a parallel Medulla-side copy would only
//! drift.
//!
//! Runs are recorded rather than merely streamed, so a workflow that paused for
//! approval or died with the process can be found again by id.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tinyflows::model::WorkflowGraph;

/// A workflow's stable identifier: the `id` in its document, defaulting to the
/// filename stem when the document omits one.
pub type WorkflowId = String;

/// One run's identifier. Doubles as the engine checkpointer's `thread_id`, which
/// is what makes a paused run resumable across process restarts.
pub type RunId = String;

/// A stored workflow: the engine graph plus where this host found it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowRecord {
    /// The workflow's stable id.
    pub id: WorkflowId,
    /// Display name; falls back to the id when the document omits one.
    pub name: String,
    /// Operator-facing description of what the workflow does.
    #[serde(default)]
    pub description: String,
    /// Whether the workflow may be run. A disabled workflow still lists and
    /// validates, so an operator can repair one without it firing.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// The engine graph.
    pub graph: WorkflowGraph,
    /// The file this record was read from, when it came from disk. `None` for a
    /// graph built in memory (an agent's draft, an import not yet saved).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_path: Option<PathBuf>,
}

/// Workflows are enabled unless a document says otherwise.
fn default_enabled() -> bool {
    true
}

impl WorkflowRecord {
    /// The listing view of this record.
    pub fn summary(&self) -> WorkflowSummary {
        WorkflowSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            enabled: self.enabled,
            node_count: self.graph.nodes.len(),
            trigger_kind: self.trigger_kind(),
        }
    }

    /// The graph's trigger kind, as a lowercase string.
    ///
    /// Read out of the trigger node's free-form config rather than a typed
    /// field, because that is where the engine keeps it. `None` when the graph
    /// has no single trigger — which validation will also report, so this stays
    /// quiet rather than duplicating the error.
    pub fn trigger_kind(&self) -> Option<String> {
        let trigger = self.graph.trigger()?;
        trigger
            .config
            .get("trigger_kind")
            .and_then(|value| value.as_str())
            .map(str::to_string)
    }
}

/// A workflow reduced to what a list needs — the shape advertised to the
/// orchestrator and rendered in the TUI, so neither has to hold whole graphs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSummary {
    /// The workflow's stable id.
    pub id: WorkflowId,
    /// Display name.
    pub name: String,
    /// Operator-facing description.
    pub description: String,
    /// Whether the workflow may be run.
    pub enabled: bool,
    /// How many nodes the graph has.
    pub node_count: usize,
    /// The trigger kind, when the graph declares exactly one trigger.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_kind: Option<String>,
}

/// A copy of a workflow from before it was last written over.
///
/// Kept so an operator can disagree with an edit after the fact. That matters
/// most for the copilot, which writes to the store directly and would otherwise
/// leave a misread instruction as the only surviving version of a graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRevision {
    /// This snapshot's id, unique within its workflow. Sorts chronologically.
    pub id: String,
    /// Epoch-millisecond stamp of when this copy stopped being current.
    ///
    /// When it was *superseded*, not when it was authored — a revision is
    /// named by the edit that replaced it, which is what an operator scanning
    /// history is looking for.
    pub superseded_at: u64,
    /// The workflow as it was.
    pub record: WorkflowRecord,
}

/// Where a run got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Started and not yet settled.
    Running,
    /// Parked on one or more approval gates; resumable.
    PendingApproval,
    /// Finished successfully.
    Succeeded,
    /// Finished with an error.
    Failed,
    /// Cancelled by an operator or an abort frame.
    Cancelled,
    /// The process went away mid-run. Reconciled from `Running` on drop, so a
    /// crashed run is never left claiming to be live.
    Interrupted,
}

impl RunStatus {
    /// Whether this status is terminal — no resume or cancel applies.
    pub fn is_settled(&self) -> bool {
        !matches!(self, Self::Running | Self::PendingApproval)
    }
}

/// One node's execution within a run, recorded as the engine reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStep {
    /// The node this step ran.
    pub node_id: String,
    /// The engine's step status, lowercased.
    pub status: String,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u128,
    /// Expressions that resolved to null, which are usually a wiring mistake
    /// rather than an intended value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

/// A durable record of one workflow run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    /// This run's id, and the checkpointer thread id that can resume it.
    pub id: RunId,
    /// The workflow that ran.
    pub workflow_id: WorkflowId,
    /// Where the run got to.
    pub status: RunStatus,
    /// Epoch-millisecond start stamp.
    pub started_at: u64,
    /// Epoch-millisecond settle stamp, absent while running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    /// Steps in completion order.
    #[serde(default)]
    pub steps: Vec<RunStep>,
    /// Node ids currently awaiting approval. Non-empty exactly when the status
    /// is [`RunStatus::PendingApproval`], and the set a resume must name.
    #[serde(default)]
    pub pending_approvals: Vec<String>,
    /// Failure message, when the run failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What can go wrong reading, writing, or running a workflow.
#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    /// No workflow with that id is known to the store.
    #[error("no workflow with id '{0}'")]
    NotFound(WorkflowId),

    /// No run with that id is known to the store.
    #[error("no run with id '{0}'")]
    RunNotFound(RunId),

    /// The graph did not pass the engine's validation. Carries every failure,
    /// not just the first, so one round-trip tells an author everything.
    #[error("workflow '{id}' is invalid: {}", .messages.join("; "))]
    Invalid {
        /// The workflow that failed validation.
        id: WorkflowId,
        /// One message per validation failure.
        messages: Vec<String>,
    },

    /// A document could not be read or parsed.
    #[error("{0}")]
    Malformed(String),

    /// The filesystem refused an operation.
    #[error("{path}: {source}")]
    Io {
        /// The path being operated on.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },

    /// The engine refused to compile or run the graph.
    #[error("{0}")]
    Engine(String),

    /// A dispatch to a harness ran out of time before it replied.
    ///
    /// Kept apart from the three below because the operator's next step differs
    /// for each: a timeout is worth retrying, an abort was deliberate, a harness
    /// error wants reading, and an unreachable harness wants configuring.
    #[error("the harness did not respond in time")]
    DispatchTimeout,

    /// A dispatch was aborted before it replied.
    #[error("the turn was aborted")]
    DispatchAborted,

    /// The harness ran and reported a failure of its own.
    #[error("harness: {0}")]
    Harness(String),

    /// The dispatch never reached a harness — no transport, no worker, or the
    /// waiter went away.
    #[error("could not reach a harness: {0}")]
    Unreachable(String),
}

impl From<crate::hub::RunError> for WorkflowError {
    /// Preserve the shape of a dispatch failure rather than flattening it.
    ///
    /// The hub already distinguishes these four; collapsing them into one
    /// string made a missing harness and a deliberate abort read identically at
    /// every call site above.
    fn from(err: crate::hub::RunError) -> Self {
        use crate::hub::RunError;
        match err {
            RunError::Timeout => Self::DispatchTimeout,
            RunError::Aborted => Self::DispatchAborted,
            RunError::Worker(message) => Self::Harness(message),
            RunError::Transport(message) => Self::Unreachable(message),
        }
    }
}
