//! Turning a run's work events into MCP progress notifications.
//!
//! A workflow run already narrates itself: the observer in
//! [`crate::flow_engine::observability`] emits a plan when the run starts, a
//! rewritten todo list every time a node settles, and a result when it ends.
//! The TUI draws those. A waiting MCP client wants the same events said in one
//! line each, so it knows the call is alive and roughly where it has got to.
//!
//! Only the three run-shaped kinds are forwarded. A workflow that dispatches
//! ten agents emits file changes and sub-agent starts too, and relaying those
//! would turn a heartbeat into a firehose in the client's transcript.

use serde_json::Value;

use crate::flow_engine::WorkEventSink;
use crate::harness_work::kinds;
use crate::mcp::progress::Progress;

/// A sink that reports a run's shape and its settling steps to `progress`.
pub(crate) fn sink(progress: Progress, workflow_id: &str) -> WorkEventSink {
    let workflow_id = workflow_id.to_string();
    std::sync::Arc::new(move |kind: &str, payload: Value| {
        let Some((message, total)) = line(&workflow_id, kind, &payload) else {
            return;
        };
        progress.report(message, total);
    })
}

/// The one line this event is worth, and the run's node count when it says.
///
/// `None` for a kind that is not part of the run's own narration.
fn line(workflow_id: &str, kind: &str, payload: &Value) -> Option<(String, Option<u64>)> {
    match kind {
        // The plan, once, at the start: how many nodes there are to get through.
        kinds::PLAN_UPDATE => {
            let total = steps(payload).len() as u64;
            Some((
                format!("{workflow_id}: started, {total} step(s) to run"),
                Some(total),
            ))
        }
        // A node settled and the plan was rewritten. This is the heartbeat that
        // matters: it fires once per step, and a step is the unit a workflow is
        // slow in.
        kinds::TODO_UPDATE => {
            let steps = steps(payload);
            let total = steps.len() as u64;
            let done = steps
                .iter()
                .filter(|step| status(step) == "completed")
                .count() as u64;
            let running = steps
                .iter()
                .find(|step| status(step) == "in_progress")
                .and_then(label)
                .map(|label| format!(", running '{label}'"))
                .unwrap_or_default();
            Some((
                format!("{workflow_id}: {done}/{total} step(s) done{running}"),
                Some(total),
            ))
        }
        kinds::RUN_RESULT => {
            let summary = payload
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("finished");
            Some((format!("{workflow_id}: {summary}"), None))
        }
        _ => None,
    }
}

/// The plan entries in a `plan_update` or `todo_update` payload.
///
/// The two kinds spell the same list under different keys — `steps` and
/// `todos` — because they descend from different harness events.
fn steps(payload: &Value) -> &[Value] {
    payload
        .get("steps")
        .or_else(|| payload.get("todos"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// One plan entry's status, or the empty string when it has none.
fn status(step: &Value) -> &str {
    step.get("status").and_then(Value::as_str).unwrap_or("")
}

/// One plan entry's label: the node's name, or its id when unnamed.
fn label(step: &Value) -> Option<&str> {
    step.get("content").and_then(Value::as_str)
}

#[cfg(test)]
#[path = "run_progress_tests.rs"]
mod tests;
