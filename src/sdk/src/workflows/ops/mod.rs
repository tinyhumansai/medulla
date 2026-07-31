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
//!
//! The split is by what an operation touches: [`graph`] for the workflow
//! document and the host facts an author needs to write one, [`runs`] for
//! executing it and reading back what happened, and [`evolve`] for what the
//! host has learned about it and what it suggests changing.

mod evolve;
mod graph;
mod runs;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};
use tinyflows::graph_ops::GraphOp;

use crate::workflows::{FileWorkflowStore, WorkflowError, WorkflowRecord, WorkflowStore};

pub use evolve::{
    accept_proposal, add_note, evolve, notes, proposals, propose, reject_proposal, verify_proposal,
};
pub use graph::{
    apply_ops, catalog, create, delete, get, host_facts, list, preview_ops, set_defaults, validate,
};
pub use runs::{cancel_run, dry_run, get_run, list_history, list_runs, rollback, run, undo};

/// The store every operation reads and writes, discovered for this environment.
pub fn discover_store(env: &HashMap<String, String>, cwd: &Path) -> Arc<dyn WorkflowStore> {
    Arc::new(FileWorkflowStore::discover(env, cwd))
}

/// A record as the document an author sees: the graph, with the host fields
/// beside it.
fn record_value(record: &WorkflowRecord) -> Value {
    json!({
        "id": record.id,
        "name": record.name,
        "description": record.description,
        "enabled": record.enabled,
        "defaults": record.defaults,
        "graph": record.graph,
    })
}

/// Parse the patch language, accepting either a bare array or `{ "ops": [...] }`.
///
/// Both spellings turn up in practice — a model told "pass the ops" sends the
/// array, one told "call with an ops argument" sends the object — and rejecting
/// either costs a round-trip to learn nothing.
pub(crate) fn parse_ops(value: &Value) -> Result<Vec<GraphOp>, WorkflowError> {
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
