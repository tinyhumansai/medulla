//! Operations that execute a workflow or read back what an execution did,
//! plus the edit-history operations that undo one.
//!
//! History lives here rather than beside the document operations because it is
//! read for the same reason a run record is: to work out what happened and go
//! back from it.

mod types;
pub mod view;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};

use super::record_value;
use crate::workflows::local::LocalRun;
use crate::workflows::{StoreWorkflowResolver, WorkflowError, WorkflowId, WorkflowStore};

pub use types::Wait;
pub use view::StepDetail;

/// Simulate a workflow: resolve every expression and satisfy every declared
/// output shape, without dispatching to a harness or touching the network.
pub async fn dry_run(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    input: Value,
    inputs: Map<String, Value>,
) -> Result<Value, WorkflowError> {
    // A dry run never dispatches a real harness session, so a loop iterates
    // at simulation cost only — nothing here needs the host's ceiling.
    let resolver = Arc::new(StoreWorkflowResolver::new(store.clone(), u64::MAX));
    let result = crate::workflows::run::dry_run(store.clone(), resolver, id, input, inputs).await?;

    // `ok` is about the *diagnosis*, not about whether the engine returned. A
    // graph whose only binding resolved to null completes perfectly and does
    // nothing, so reporting that as `ok: true` is the single most misleading
    // thing this surface could say.
    Ok(json!({
        "ok": result.diagnosis.is_clean(),
        "output": result.output,
        "diagnostics": result.diagnosis,
    }))
}

/// A workflow's run history, newest first.
///
/// `detail` decides how much of each run's steps comes back. A history listing
/// is read to find *which* run to look at, so the caller that wants a hundred
/// of them should not be paying for a hundred agent transcripts to do it.
pub fn list_runs(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    detail: StepDetail,
) -> Result<Value, WorkflowError> {
    let runs = store
        .list_runs(id)?
        .iter()
        .map(|record| view::project(record, detail))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({ "runs": runs }))
}

/// Run a workflow on this machine, for real.
///
/// Unlike [`dry_run`], this dispatches actual harness sessions, runs actual
/// scripts, and makes whatever changes the graph describes. Refused when the
/// host or the workflow is disabled — the two switches an operator has.
///
/// **Async by default.** A real workflow outlives any request timeout worth
/// setting: a three-pass babysit measured 35 minutes, one step of it 20. So
/// [`Wait::No`] returns as soon as the run is admitted, with the run id to poll
/// [`get_run`] with, and the run continues in the background. Blocking is still
/// available for the short workflows where it is honest — a dry-run-sized graph,
/// a test — under [`Wait::Forever`], or bounded with [`Wait::Until`], which
/// answers with the run id rather than an error when its budget runs out.
///
/// Everything that can be refused cheaply is still refused synchronously, in
/// every mode: a disabled workflow, an unknown one, a host that cannot start a
/// daemon. Only a run that genuinely began comes back as a run id.
///
/// A blocking wait returns the whole run record: the caller is usually a model
/// deciding what to fix next, and every step's status and diagnostics are what
/// that decision needs.
pub async fn run(request: LocalRun<'_>, wait: Wait) -> Result<Value, WorkflowError> {
    let started = request.start().await?;
    // Kept because every branch that does not produce a record answers with
    // them, and the branches that wait consume the handle to find that out.
    let run_id = started.run_id.clone();
    let workflow_id = started.workflow_id.clone();
    let settled = match wait {
        Wait::No => None,
        Wait::Forever => Some(started.settled().await?),
        Wait::Until(budget) => match started.settled_within(budget).await? {
            Some(record) => Some(record),
            None => return Ok(admitted(&run_id, &workflow_id, Some(budget))),
        },
    };
    let Some(record) = settled else {
        return Ok(admitted(&run_id, &workflow_id, None));
    };
    Ok(json!({
        "ok": record.status == crate::workflows::RunStatus::Succeeded,
        "run": record,
    }))
}

/// What a caller that is not holding the call open gets back.
///
/// `ok: true` says the run *started*, which is the only question this answer is
/// in a position to settle, and the wording says so — a model that read `ok`
/// as "it worked" would otherwise report success for a run still in its first
/// step. `waited` is present only when a budget expired, because "I gave up
/// after 30s" and "I never waited" lead to different next moves.
fn admitted(run_id: &str, workflow_id: &str, waited: Option<Duration>) -> Value {
    let mut value = json!({
        "ok": true,
        "runId": run_id,
        "workflowId": workflow_id,
        "status": "running",
        "note": format!(
            "the run started and is still going; it does not need this call to stay open. Poll \
             workflow_run_get with runId '{run_id}' for its status, and again when it is settled \
             for what it did."
        ),
    });
    if let Some(waited) = waited {
        value["waitedMs"] = json!(waited.as_millis() as u64);
    }
    value
}

/// A workflow's edit history: the versions it has been written over, newest
/// first.
///
/// The graph of each version is included. History is read to decide *which*
/// version to go back to, and a listing of opaque ids and timestamps does not
/// support that decision.
///
/// Reachable for a workflow `workflow_delete` just removed, not only a live
/// one: `delete` captures a revision of what it removed (see
/// `FileWorkflowStore::delete`), and `undo`/`rollback` restore from a revision
/// without needing the live record either — gating this on the live record
/// alone would report a deleted workflow as `NotFound` in the one moment an
/// operator most needs to see what it can still be recovered from.
pub fn list_history(store: &Arc<dyn WorkflowStore>, id: &str) -> Result<Value, WorkflowError> {
    let revisions = store.list_revisions(id)?;
    // Still an error for an id that was never anything — a truly unknown
    // workflow has neither a live record nor any revision to report.
    if revisions.is_empty() && store.get(id)?.is_none() {
        return Err(WorkflowError::NotFound(WorkflowId::from(id)));
    }
    Ok(json!({ "revisions": revisions }))
}

/// Restore a workflow to one of its earlier versions.
///
/// The restore goes through the normal save, so the version being replaced is
/// snapshotted in turn: a rollback is itself in the history and can be rolled
/// back.
pub fn rollback(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    revision_id: &str,
) -> Result<Value, WorkflowError> {
    let record = crate::workflows::store::rollback(store.as_ref(), id, revision_id)?;
    Ok(json!({
        "restored": record.id,
        "revision": revision_id,
        "workflow": record_value(&record),
    }))
}

/// Undo a workflow's most recent edit.
///
/// What an operator's undo key calls. A workflow with no history is not a
/// failure — pressing undo on something never edited is a normal thing to do —
/// so it comes back as `undone: false` with a reason.
pub fn undo(store: &Arc<dyn WorkflowStore>, id: &str) -> Result<Value, WorkflowError> {
    match crate::workflows::store::undo_last(store.as_ref(), id)? {
        Some((revision, record)) => Ok(json!({
            "undone": true,
            "revision": revision.id,
            "supersededAt": revision.superseded_at,
            "workflow": record_value(&record),
        })),
        None => Ok(json!({
            "undone": false,
            "id": id,
            "reason": "this workflow has not been edited since it was created, so there is \
                       nothing to go back to",
        })),
    }
}

/// One run record, at the level of step detail `detail` asks for.
///
/// The polling half of an async [`run`]: a run id in, what that run has done so
/// far out — including while it is still going, because the record is written
/// as `running` the moment the run is admitted.
pub fn get_run(
    store: &Arc<dyn WorkflowStore>,
    run_id: &str,
    detail: StepDetail,
) -> Result<Value, WorkflowError> {
    let record = crate::workflows::require_run(store.as_ref(), run_id)?;
    view::project(&record, detail)
}

/// Cancel a run executing in *this* process.
///
/// The registry is process-local, so a `medulla workflow cancel` typed in one
/// shell cannot reach a run started in another: there is no control channel
/// between two CLI invocations. Rather than report a bare `false` that reads as
/// "cancelled nothing, all good", the result says which case it was, so a
/// caller can tell "already finished" from "not mine to cancel".
///
/// The paths that *can* always cancel are the ones that own the running
/// process: the TUI cancels the run it started, and an orchestrator's abort
/// frame reaches the daemon that is executing it.
pub fn cancel_run(run_id: &str) -> Value {
    if crate::workflows::run::cancel(run_id) {
        return json!({ "cancelled": true, "runId": run_id });
    }
    json!({
        "cancelled": false,
        "runId": run_id,
        "reason": "no run with this id is executing in this process; a run started by another \
                   process must be cancelled where it runs (the TUI that started it, or an \
                   orchestrator abort to the daemon executing it)",
    })
}
