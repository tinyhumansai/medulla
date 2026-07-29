//! One execution's durable record.
//!
//! Unlike a [`super::WorkflowRecord`], a run is written once and never revised,
//! so it needs no snapshot ring. It is the only durable evidence of what the
//! engine actually did, which is why every field here is additive: readers of
//! run files written by an older build must keep working.

use serde::{Deserialize, Serialize};

use super::workflow::WorkflowId;
use crate::workflows::run::diagnose::Diagnosis;

/// One run's identifier. Doubles as the engine checkpointer's `thread_id`, which
/// is what makes a paused run resumable across process restarts.
pub type RunId = String;

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
