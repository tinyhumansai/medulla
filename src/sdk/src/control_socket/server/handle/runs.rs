//! The `run.report` op: a granted process saying what a workflow run it is
//! executing is doing.
//!
//! Like [`super::hooks`], this op carries no authority — it updates a bounded,
//! in-memory view of work already happening in another process. So the work
//! here is attribution and *bounding*: a report is filed under the grant's own
//! session rather than one the caller names, and every field it retains is
//! clipped before it reaches the registry.

use serde_json::{json, Value};

use super::super::super::grants::Grant;
use super::super::super::runs::{HarnessRunRegistry, HarnessRunStatus, RunReport};
use super::super::super::types::{ControlFailure, ErrorKind};

use super::params::required_str;

/// The longest identifier or node name a report may retain.
///
/// `detail` has [`crate::logging::preview`] as its backstop, but `runId`,
/// `workflowId`, and `node` are copied straight out of caller-supplied JSON.
/// The registry holds eight runs per session and two hundred frames per run,
/// each frame keeping its own `node`, so an unbounded field here multiplies
/// into a comfortable way to pin client-supplied memory well inside the 1 MiB
/// frame limit.
const MAX_FIELD: usize = 256;

/// Record what a granted process's workflow run is doing.
///
/// Keyed by the grant's *session* rather than by the token, so a child grant
/// minted for a nested session reports under the harness an operator can see —
/// the depth-1 session is an implementation detail of the run, not a second row
/// in the rail.
///
/// Always accepted while the grant is valid: this is a view of work already
/// happening elsewhere, and refusing a report would not stop the run, only hide
/// it.
///
/// # Errors
///
/// [`ErrorKind::BadRequest`] when the report names no run or no workflow.
pub(super) fn run_report(
    runs: &HarnessRunRegistry,
    grant: &Grant,
    params: &Value,
) -> Result<Value, ControlFailure> {
    let run_id = clipped(required_str(params, "runId")?, "runId")?;
    let workflow_id = clipped(required_str(params, "workflowId")?, "workflowId")?;
    let status =
        HarnessRunStatus::from_wire(params.get("status").and_then(Value::as_str).unwrap_or(""));
    let detail = params
        .get("detail")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
        .map(crate::logging::preview);
    let node = params
        .get("node")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|node| !node.is_empty())
        .map(str::to_string)
        .map(|node| clipped(node, "node"))
        .transpose()?;
    runs.report(
        &grant.session,
        RunReport {
            run_id,
            workflow_id,
            status,
            detail,
            node,
        },
    );
    Ok(json!({ "recorded": true }))
}

/// Refuse a field longer than [`MAX_FIELD`] bytes.
///
/// Refused rather than truncated, unlike `detail`: these are identifiers, and a
/// silently shortened run id would key a row the Workflows page can never look
/// up. A reporter sending one that long is broken, not verbose.
fn clipped(value: String, field: &str) -> Result<String, ControlFailure> {
    if value.len() > MAX_FIELD {
        return Err(ControlFailure::new(
            ErrorKind::BadRequest,
            format!("{field} is longer than {MAX_FIELD} bytes"),
        ));
    }
    Ok(value)
}

#[cfg(test)]
#[path = "runs_tests.rs"]
mod tests;
