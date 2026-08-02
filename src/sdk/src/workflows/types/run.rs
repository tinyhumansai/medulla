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
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    if serialized.len() <= MAX_EVIDENCE_BYTES {
        return value.clone();
    }
    // The preview is itself embedded in JSON, so reserve half the budget for
    // escaping plus the wrapper metadata. Quotes and backslashes can nearly
    // double when serialized a second time.
    let preview_budget = MAX_EVIDENCE_BYTES / 2 - 256;
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
        .map(|body| body.len() <= MAX_EVIDENCE_BYTES)
        .unwrap_or(false));
    bounded
}

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
