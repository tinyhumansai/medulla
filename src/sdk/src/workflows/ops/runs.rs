//! Operations that execute a workflow or read back what an execution did,
//! plus the edit-history operations that undo one.
//!
//! History lives here rather than beside the document operations because it is
//! read for the same reason a run record is: to work out what happened and go
//! back from it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};

use super::record_value;
use crate::workflows::{StoreWorkflowResolver, WorkflowError, WorkflowId, WorkflowStore};

/// Simulate a workflow: resolve every expression and satisfy every declared
/// output shape, without dispatching to a harness or touching the network.
pub async fn dry_run(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    input: Value,
) -> Result<Value, WorkflowError> {
    let resolver = Arc::new(StoreWorkflowResolver::new(store.clone()));
    let result = crate::workflows::run::dry_run(store.clone(), resolver, id, input).await?;

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
pub fn list_runs(store: &Arc<dyn WorkflowStore>, id: &str) -> Result<Value, WorkflowError> {
    Ok(json!({ "runs": store.list_runs(id)? }))
}

/// Run a workflow on this machine, for real.
///
/// Unlike [`dry_run`], this dispatches actual harness sessions, runs actual
/// scripts, and makes whatever changes the graph describes. Refused when the
/// host or the workflow is disabled — the two switches an operator has.
///
/// Returns the whole run record rather than a summary: the caller is usually a
/// model deciding what to fix next, and every step's status and diagnostics are
/// what that decision needs.
pub async fn run(
    store: &Arc<dyn WorkflowStore>,
    config: &crate::config::WorkflowsConfig,
    custom_harnesses: &[crate::config::CustomHarnessConfig],
    env: &HashMap<String, String>,
    cwd: &Path,
    id: &str,
    input: Value,
) -> Result<Value, WorkflowError> {
    let record = crate::workflows::local::run_here(
        store.clone(),
        config,
        custom_harnesses,
        env,
        cwd,
        id,
        input,
    )
    .await?;
    Ok(json!({
        "ok": record.status == crate::workflows::RunStatus::Succeeded,
        "run": record,
    }))
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

/// One run record.
pub fn get_run(store: &Arc<dyn WorkflowStore>, run_id: &str) -> Result<Value, WorkflowError> {
    let record = crate::workflows::require_run(store.as_ref(), run_id)?;
    serde_json::to_value(record)
        .map_err(|err| WorkflowError::Engine(format!("could not serialize run '{run_id}': {err}")))
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
