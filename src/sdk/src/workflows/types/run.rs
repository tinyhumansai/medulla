//! One execution's durable record.
//!
//! Unlike a [`super::WorkflowRecord`], a run is written once and never revised,
//! so it needs no snapshot ring. It is the only durable evidence of what the
//! engine actually did, which is why every field here is additive: readers of
//! run files written by an older build must keep working.

use serde::{Deserialize, Serialize};

use super::workflow::WorkflowId;
use crate::workflows::run::diagnose::Diagnosis;

/// Maximum serialized bytes retained for one step input or output.
pub(crate) const MAX_EVIDENCE_BYTES: usize = 64 * 1024;

/// Keep small evidence intact and summarize values that would bloat history.
///
/// Execution and diagnosis retain the engine's full in-memory value. Only the
/// durable inspection copy is bounded, so one response cannot make every
/// future history listing read an arbitrarily large file.
pub(crate) fn bounded_evidence(value: &serde_json::Value) -> serde_json::Value {
    bounded_within(value, MAX_EVIDENCE_BYTES)
}

/// Keep small values intact and summarize ones larger than `max_bytes`.
///
/// The same bounding as [`bounded_evidence`] against a caller-chosen budget.
/// The durable record uses a generous one because it is written once; a reply
/// projected for a model uses a much smaller one, because a hundred of them
/// land in the same context window.
///
/// The wrapper shape is deliberately identical at every budget, so a reader
/// that knows how to unpack a truncated run file already knows how to unpack a
/// truncated reply.
pub(crate) fn bounded_within(value: &serde_json::Value, max_bytes: usize) -> serde_json::Value {
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    if serialized.len() <= max_bytes {
        return value.clone();
    }
    // The preview is itself embedded in JSON, so reserve half the budget for
    // escaping plus the wrapper metadata. Quotes and backslashes can nearly
    // double when serialized a second time.
    let preview_budget = (max_bytes / 2).saturating_sub(256);
    let end = serialized
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= preview_budget)
        .last()
        .unwrap_or(0);
    let bounded = serde_json::json!({
        "_medullaTruncated": true,
        "originalBytes": serialized.len(),
        "preview": &serialized[..end],
    });
    debug_assert!(serde_json::to_vec(&bounded)
        .map(|body| body.len() <= max_bytes.max(512))
        .unwrap_or(false));
    bounded
}

/// One run's identifier. Doubles as the engine checkpointer's `thread_id`, which
/// is what makes a paused run resumable across process restarts.
pub type RunId = String;

/// Bytes of one declared input value kept on the durable record.
///
/// Much smaller than [`MAX_EVIDENCE_BYTES`]: an input is a knob a caller turned,
/// and every surface that shows a run shows all of them at once. A repository
/// name, a branch, a PR number, a paragraph of instruction all fit; a pasted
/// transcript is summarized rather than carried into every future listing.
pub(crate) const MAX_INPUT_BYTES: usize = 4 * 1024;

/// Who asked for a run, and from where.
///
/// Recorded because a run record on its own cannot say why it exists. The
/// question an operator asks in front of a rail full of runs is "which of these
/// did the session I am sitting in start", and answering it needs the session's
/// own correlation key on the record — nothing about the workflow, the graph, or
/// the steps can supply it after the fact.
///
/// Every field is optional except `kind`, because the callers differ in how much
/// they know about themselves: a `medulla workflow run` on a terminal knows it
/// is a CLI and nothing else, while a harness session knows the key its tool
/// grant was minted under.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOrigin {
    /// What kind of caller started the run — see [`RunOrigin::SESSION`] and its
    /// siblings.
    ///
    /// A free string rather than an enum: a build that learns a new door must
    /// still be readable by one that does not, and an unknown kind displayed
    /// verbatim is strictly better than a record that fails to parse.
    pub kind: String,
    /// The harness session this run was started from, when one was.
    ///
    /// The MCP grant key the session's tool server was launched under
    /// (`pty-<uuid>`), which is the only identifier shared by the harness
    /// process and the host that spawned it. This is what nests a run under its
    /// session in the Agents rail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// What to call the caller on screen, when it has a name worth showing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The directory the run was started in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

impl RunOrigin {
    /// A run started by a harness session through the workflow MCP tools.
    pub const SESSION: &'static str = "session";
    /// A run started from a terminal by `medulla workflow run`.
    pub const CLI: &'static str = "cli";
    /// A run started from the operator's own Workflows pane.
    pub const OPERATOR: &'static str = "operator";

    /// An origin naming the harness session `session` started the run.
    pub fn session(session: impl Into<String>) -> Self {
        Self {
            kind: Self::SESSION.to_string(),
            session: Some(session.into()),
            ..Self::default()
        }
    }

    /// An origin of `kind` with nothing else known about it.
    pub fn of_kind(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            ..Self::default()
        }
    }

    /// Record the directory the run started in.
    pub fn in_workspace(mut self, workspace: impl Into<String>) -> Self {
        let workspace = workspace.into();
        self.workspace = (!workspace.trim().is_empty()).then_some(workspace);
        self
    }

    /// Record a display name for the caller.
    pub fn labelled(mut self, label: impl Into<String>) -> Self {
        let label = label.into();
        self.label = (!label.trim().is_empty()).then_some(label);
        self
    }
}

/// The OS process that is executing a run.
///
/// A run record on its own cannot say whether the process that wrote it is
/// still alive, which is the difference between "another medulla is working on
/// this" and "this row is a tombstone from a build that was killed". Stamping
/// the executor turns that unanswerable question into a liveness check.
///
/// Both halves of the identity matter. A bare pid is not enough: pids are
/// recycled, so a run left behind by pid 4711 would look alive the moment
/// something unrelated is assigned 4711. `started_at_secs` — the executing
/// process's own start time — makes the pair unique for as long as the record
/// is worth reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunExecutor {
    /// The host the process runs on.
    ///
    /// The run store is per-machine today, but a record naming a pid with no
    /// host is one shared filesystem away from a liveness check that compares
    /// this machine's process table against another machine's pid. Recording it
    /// costs nothing and makes the check refuse rather than guess.
    pub host: String,
    /// The executing process's id.
    pub pid: u32,
    /// When that process itself started, in seconds since the epoch.
    ///
    /// The pid-reuse guard. Absent when the platform would not report it, in
    /// which case a liveness check falls back to the pid alone and errs toward
    /// leaving the record alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_secs: Option<u64>,
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
    /// The resolved input this activation received.
    ///
    /// Currently recorded for agent nodes as their full prompt. Absent on
    /// other node kinds and on records written before input evidence existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    /// The items emitted by this activation, retained for run inspection.
    ///
    /// Absent on records written before step results were persisted and null
    /// when the engine failed before producing an output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
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
    /// The values supplied for the workflow's *declared* inputs, by name.
    ///
    /// The single most useful thing about a run that the graph cannot supply:
    /// two runs of one workflow differ only in what was passed to them, so a
    /// history that omitted this listed the same sentence over and over. Bounded
    /// per value ([`MAX_INPUT_BYTES`]) rather than whole, because every surface
    /// that lists runs shows all of a run's inputs at once.
    ///
    /// Empty on a workflow that declares no inputs, and on records written
    /// before this field existed — the two are indistinguishable, which is
    /// acceptable: neither has anything to show.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub inputs: serde_json::Map<String, serde_json::Value>,
    /// The free-form trigger payload the run was started with.
    ///
    /// Separate from [`inputs`](Self::inputs) because they are separate
    /// arguments: `inputs` names the workflow's declared parameters, while this
    /// is whatever the trigger handed the graph. Absent when it was empty, and
    /// on records written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<serde_json::Value>,
    /// Who started this run, and from where.
    ///
    /// Absent on records written before this field existed, and on a run whose
    /// caller could not say anything about itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<RunOrigin>,
    /// The process executing this run, while one is.
    ///
    /// Written when the run is admitted and left in place when it settles: a
    /// finished run's executor is history, not a claim. What reads it is the
    /// reconciliation sweep, which only consults it for a record that still
    /// says [`RunStatus::Running`].
    ///
    /// Absent on records written before this field existed, which the sweep
    /// treats as unowned — an old record claiming to run is exactly the kind
    /// this exists to clean up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<RunExecutor>,
    /// Whether someone has asked this run to stop.
    ///
    /// The durable half of cancellation. The in-process registry can only reach
    /// a run this process is executing, so a cancel aimed at a run owned by
    /// another live medulla is written here instead; that process notices the
    /// flag on its next poll and cancels itself. Never cleared — a run that was
    /// asked to stop and then settled should still say it was asked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cancel_requested: bool,
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
    /// One line saying what this run did, written when it settled.
    ///
    /// The observer builds this to narrate the run live; keeping it means a
    /// reader after the fact — an operator scanning history, an agent reviewing
    /// what a workflow has been doing — gets the same sentence rather than
    /// re-deriving a worse one from the steps.
    ///
    /// Absent on records written before this field existed, and on runs that
    /// never settled through the engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// What was wrong with the run beyond whether it failed.
    ///
    /// Null bindings, errors an `on_error` policy swallowed, and nodes that
    /// never executed. Previously produced only for *dry* runs, which meant the
    /// runs that actually mattered were the ones with no diagnosis at all.
    ///
    /// Absent on records written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnosis: Option<Diagnosis>,
}

impl RunRecord {
    /// Record what this run was started with.
    ///
    /// Values are bounded individually rather than as one blob, so a single
    /// oversized argument does not summarize away the six small ones beside it —
    /// which are usually the ones that identify the run.
    pub fn with_inputs(
        mut self,
        inputs: &serde_json::Map<String, serde_json::Value>,
        trigger: &serde_json::Value,
    ) -> Self {
        self.inputs = inputs
            .iter()
            .map(|(name, value)| (name.clone(), bounded_within(value, MAX_INPUT_BYTES)))
            .collect();
        // An empty trigger is what almost every caller passes, and recording
        // `{}` on every run would put a meaningless row on every run view.
        self.trigger = match trigger {
            serde_json::Value::Null => None,
            serde_json::Value::Object(map) if map.is_empty() => None,
            value => Some(bounded_within(value, MAX_INPUT_BYTES)),
        };
        self
    }

    /// Record who asked for this run.
    pub fn with_origin(mut self, origin: Option<RunOrigin>) -> Self {
        self.origin = origin;
        self
    }

    /// Record the process executing this run.
    pub fn with_executor(mut self, executor: Option<RunExecutor>) -> Self {
        self.executor = executor;
        self
    }

    /// How long the run took, once it has settled.
    pub fn duration_ms(&self) -> Option<u64> {
        self.finished_at
            .map(|finished| finished.saturating_sub(self.started_at))
    }

    /// How many recorded steps ended in an engine-reported failure.
    pub fn failed_steps(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| {
                matches!(
                    step.status.trim().to_ascii_lowercase().as_str(),
                    "failed" | "error"
                )
            })
            .count()
    }
}
