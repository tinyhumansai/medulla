//! A one-line account of how a run ended.
//!
//! The engine's observer writes its own summary when a run settles through it
//! (see [`crate::flow_engine::observability::WorkflowRunObserver::summary`]),
//! and that one is preferred because it can see how many steps the engine
//! actually ran. This is the fallback for the runs it never sees: cancelled,
//! timed out, or reconciled after the process went away.
//!
//! Deliberately the only other place a run is put into words. When the daemon
//! phrased its reply frames separately, the same run could be described two
//! different ways depending on who was reading it.

use crate::workflows::{RunRecord, RunStatus};

/// Describe a settled run in one line.
pub fn summarize(record: &RunRecord) -> String {
    let steps = record.steps.len();
    match record.status {
        RunStatus::Succeeded => format!("workflow completed {steps} steps"),
        RunStatus::PendingApproval => format!(
            "workflow paused after {steps} steps, awaiting approval: {}",
            record.pending_approvals.join(", ")
        ),
        RunStatus::Cancelled => format!("workflow cancelled after {steps} steps"),
        RunStatus::Interrupted => format!("workflow interrupted after {steps} steps"),
        RunStatus::Failed => match &record.error {
            Some(error) => format!("workflow failed after {steps} steps: {error}"),
            None => format!("workflow failed after {steps} steps"),
        },
        RunStatus::Running => format!("workflow still running after {steps} steps"),
    }
}
