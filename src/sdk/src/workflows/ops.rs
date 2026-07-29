//! The workflow operations, as one JSON-in/JSON-out surface.
//!
//! Every way of reaching workflows — the `medulla workflow` subcommand, the MCP
//! tools an ACP harness calls, the TUI — wants the same dozen operations. Having
//! them here once means those surfaces cannot drift: a harness authoring a
//! workflow over a tool call and an operator typing a command are running the
//! same code, and a fix to one is a fix to all.
//!
//! Operations return `serde_json::Value` rather than typed results because
//! their principal caller is a model reading JSON. The human-readable rendering
//! belongs to whichever surface is talking to a human.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use tinyflows::graph_ops::GraphOp;

use crate::workflows::authoring::{
    apply_workflow_ops, create_workflow, preview_workflow_ops, validate_handle, GraphHandle,
};
use crate::workflows::node_contracts::{all_node_kind_contracts, node_kind_contract};
use crate::workflows::store::require;
use crate::workflows::{
    FileWorkflowStore, StoreWorkflowResolver, WorkflowError, WorkflowRecord, WorkflowStore,
};

/// The store every operation reads and writes, discovered for this environment.
pub fn discover_store(env: &HashMap<String, String>, cwd: &Path) -> Arc<dyn WorkflowStore> {
    Arc::new(FileWorkflowStore::discover(env, cwd))
}

/// Every installed workflow, as a listing.
pub fn list(store: &Arc<dyn WorkflowStore>) -> Result<Value, WorkflowError> {
    let workflows = store.list()?;
    Ok(json!({ "workflows": workflows }))
}

/// One workflow, whole: the host fields plus the graph an author would edit.
pub fn get(store: &Arc<dyn WorkflowStore>, id: &str) -> Result<Value, WorkflowError> {
    let record = require(store.as_ref(), id)?;
    Ok(record_value(&record))
}

/// Install a workflow from a whole graph document.
pub fn create(
    store: &Arc<dyn WorkflowStore>,
    document: &str,
    id_fallback: &str,
) -> Result<Value, WorkflowError> {
    let record = create_workflow(store, document, id_fallback)?;
    Ok(json!({ "created": record.id, "workflow": record_value(&record) }))
}

/// Remove a workflow.
pub fn delete(store: &Arc<dyn WorkflowStore>, id: &str) -> Result<Value, WorkflowError> {
    store.delete(id)?;
    Ok(json!({ "deleted": id }))
}

/// Apply a batch of graph edits and save the result.
///
/// `ops` is the engine's patch language as JSON — an array of tagged objects
/// like `{"op":"update_node_config","id":"build","config":{...}}`.
pub fn apply_ops(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    ops: &Value,
) -> Result<Value, WorkflowError> {
    let ops = parse_ops(ops)?;
    let record = apply_workflow_ops(store, id, &ops)?;
    Ok(json!({ "updated": record.id, "workflow": record_value(&record) }))
}

/// Check a batch of graph edits without saving them.
pub fn preview_ops(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    ops: &Value,
) -> Result<Value, WorkflowError> {
    let ops = parse_ops(ops)?;
    let graph = preview_workflow_ops(store, id, &ops)?;
    Ok(json!({ "ok": true, "graph": graph }))
}

/// Validate a saved workflow, or an inline document that has not been saved.
///
/// Returns `ok: false` with every message rather than an error, because an
/// author asking "is this valid" has been answered either way — a failure here
/// is the result, not a fault.
pub fn validate(store: &Arc<dyn WorkflowStore>, handle: &GraphHandle<'_>) -> Value {
    match validate_handle(store, handle) {
        Ok(record) => json!({
            "ok": true,
            "id": record.id,
            "nodes": record.graph.nodes.len(),
            "edges": record.graph.edges.len(),
        }),
        Err(WorkflowError::Invalid { id, messages }) => {
            json!({ "ok": false, "id": id, "errors": messages })
        }
        Err(err) => json!({ "ok": false, "errors": [err.to_string()] }),
    }
}

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
    env: &HashMap<String, String>,
    cwd: &Path,
    id: &str,
    input: Value,
) -> Result<Value, WorkflowError> {
    let record =
        crate::workflows::local::run_here(store.clone(), config, env, cwd, id, input).await?;
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
pub fn list_history(store: &Arc<dyn WorkflowStore>, id: &str) -> Result<Value, WorkflowError> {
    // Fail on an unknown workflow rather than reporting an empty history, which
    // would read as "this has never been edited" for a workflow that does not
    // exist at all.
    require(store.as_ref(), id)?;
    Ok(json!({ "revisions": store.list_revisions(id)? }))
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
    Ok(serde_json::to_value(record).unwrap_or(Value::Null))
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
        "reason": "no run with this id is executing in this process; a run started by another                    process must be cancelled where it runs (the TUI that started it, or an                    orchestrator abort to the daemon executing it)",
    })
}

/// The node kinds an author may use, with this host's notes.
///
/// The document a model reads before writing a graph. `kind` narrows it to one
/// contract, which is what an author does once they know which node they need.
pub fn catalog(kind: Option<&str>) -> Result<Value, WorkflowError> {
    match kind {
        Some(kind) => {
            let contract = node_kind_contract(kind).ok_or_else(|| {
                WorkflowError::Engine(format!(
                    "unknown node kind '{kind}'; known kinds: {}",
                    crate::workflows::node_contracts::NODE_KINDS.join(", ")
                ))
            })?;
            Ok(json!({ "contract": contract }))
        }
        None => Ok(json!({ "contracts": all_node_kind_contracts() })),
    }
}

/// What this host will actually permit a workflow to do.
///
/// The grounding an author most needs and had no way to get. Every one of these
/// is enforced at *run* time, so a graph that ignores them saves cleanly,
/// validates cleanly, and then fails the first time it matters — usually
/// overnight, to nobody watching.
///
/// Read from configuration rather than guessed, so the answer is this machine's
/// and not a plausible default.
pub fn host_facts(config: &crate::config::WorkflowsConfig) -> Value {
    json!({
        "defaultWorker": if config.default_worker.trim().is_empty() {
            Value::Null
        } else {
            Value::String(config.default_worker.clone())
        },
        "nativeTools": crate::flow_engine::caps::tools::NATIVE_TOOLS,
        "toolAllowlist": config.tool_allowlist,
        "httpAllowlist": config.http_allowlist,
        "allowCode": config.allow_code,
        "runTimeoutSecs": config.run_timeout_secs,
        "notes": [
            if config.default_worker.trim().is_empty() {
                "No default worker is configured, so every `agent` node must name a \
                 `config.agent_ref` — a node without one fails at run time."
            } else {
                "An `agent` node with no `config.agent_ref` dispatches to the default worker \
                 above. Any other value must name a worker this host can actually reach."
            },
            "A `tool_call` slug outside `nativeTools` must appear in `toolAllowlist`, and even \
             then there is no third-party integration registry on this host — it will still \
             fail at run time. Express host-specific work as an `agent` node.",
            "Only `manual` triggers fire here. Other kinds are stored and never dispatched.",
        ],
    })
}

/// A record as the document an author sees: the graph, with the host fields
/// beside it.
fn record_value(record: &WorkflowRecord) -> Value {
    json!({
        "id": record.id,
        "name": record.name,
        "description": record.description,
        "enabled": record.enabled,
        "graph": record.graph,
    })
}

/// Parse the patch language, accepting either a bare array or `{ "ops": [...] }`.
///
/// Both spellings turn up in practice — a model told "pass the ops" sends the
/// array, one told "call with an ops argument" sends the object — and rejecting
/// either costs a round-trip to learn nothing.
fn parse_ops(value: &Value) -> Result<Vec<GraphOp>, WorkflowError> {
    let array = match value {
        Value::Array(_) => value.clone(),
        Value::Object(object) => object
            .get("ops")
            .cloned()
            .ok_or_else(|| WorkflowError::Malformed("expected an array of ops".to_string()))?,
        _ => {
            return Err(WorkflowError::Malformed(
                "expected an array of ops".to_string(),
            ))
        }
    };
    serde_json::from_value(array)
        .map_err(|err| WorkflowError::Malformed(format!("invalid ops: {err}")))
}

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
