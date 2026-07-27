//! The tools a harness sees, and the dispatch behind them.
//!
//! Names are prefixed `workflow_` so they cannot collide with the harness's own
//! reserved tool names ([`crate::harness_contract::RESERVED_TOOL_NAMES`]).
//!
//! Descriptions matter more here than anywhere else in this feature: they are
//! the whole of what a model knows before it acts. The node-kind vocabulary is
//! *generated* from the catalogue rather than written out, so the description
//! cannot drift from what the engine actually accepts.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::workflows::authoring::GraphHandle;
use crate::workflows::node_contracts::render_node_kinds_line;
use crate::workflows::{ops, WorkflowStore};

use super::RpcError;

/// Every tool this server exposes, in the order a model meets them.
pub const TOOL_NAMES: [&str; 9] = [
    "workflow_list",
    "workflow_get",
    "workflow_catalog",
    "workflow_create",
    "workflow_apply_ops",
    "workflow_preview_ops",
    "workflow_validate",
    "workflow_dry_run",
    "workflow_runs",
];

/// A JSON Schema object with the given properties and required keys.
fn schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

/// The tool definitions, as `tools/list` returns them.
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "workflow_list",
            "description":
                "List the workflows installed on this machine. Start here: a workflow is a saved \
                 multi-step plan whose `agent` steps each run on a real coding harness.",
            "inputSchema": schema(json!({}), &[]),
        }),
        json!({
            "name": "workflow_get",
            "description":
                "Fetch one workflow whole, including the graph you would edit. Call this before \
                 workflow_apply_ops so your patches target node ids that exist.",
            "inputSchema": schema(
                json!({ "id": { "type": "string", "description": "The workflow id." } }),
                &["id"],
            ),
        }),
        json!({
            "name": "workflow_catalog",
            "description": format!(
                "The node kinds a workflow may use, with this host's own notes on each. Read \
                 this before writing a graph.\n\n{}",
                render_node_kinds_line()
            ),
            "inputSchema": schema(
                json!({
                    "kind": {
                        "type": "string",
                        "description":
                            "Narrow to one node kind. Omit for every contract.",
                    }
                }),
                &[],
            ),
        }),
        json!({
            "name": "workflow_create",
            "description":
                "Install a workflow from a whole graph document. The document is a tinyflows \
                 WorkflowGraph — `nodes` and `edges`, plus optional `name`, `description`, and \
                 `enabled`. Exactly one node must be a `trigger`. An invalid graph is refused \
                 and nothing is written, so validate first if you are unsure.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "The id to install it under." },
                    "document": {
                        "type": "string",
                        "description": "The workflow graph as a JSON string.",
                    }
                }),
                &["id", "document"],
            ),
        }),
        json!({
            "name": "workflow_apply_ops",
            "description":
                "Edit a saved workflow with graph patches, and save the result. Prefer this over \
                 rewriting a whole document: each op is checked, and a batch that fails anywhere \
                 leaves the workflow untouched. Ops are objects like \
                 {\"op\":\"update_node_config\",\"id\":\"build\",\"config\":{...}} — \
                 update_node_config is an RFC 7386 merge patch, so a null leaf deletes a key. \
                 Note that rename_node rewires edges but does NOT rewrite `=nodes.<id>` \
                 expressions inside other nodes' configs; re-point those yourself.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "The workflow to edit." },
                    "ops": { "type": "array", "description": "The graph ops to apply." }
                }),
                &["id", "ops"],
            ),
        }),
        json!({
            "name": "workflow_preview_ops",
            "description":
                "Check graph patches against a saved workflow without saving them. The safe way \
                 to find out whether an edit is sound.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "The workflow to check against." },
                    "ops": { "type": "array", "description": "The graph ops to preview." }
                }),
                &["id", "ops"],
            ),
        }),
        json!({
            "name": "workflow_validate",
            "description":
                "Validate a saved workflow, or a document you have not saved yet. Reports every \
                 problem at once rather than the first, so one call tells you everything wrong. \
                 Returns ok:false with errors rather than failing.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "A saved workflow to validate." },
                    "document": {
                        "type": "string",
                        "description": "An unsaved graph document to validate instead.",
                    }
                }),
                &[],
            ),
        }),
        json!({
            "name": "workflow_dry_run",
            "description":
                "Simulate a saved workflow: every expression is resolved and every declared \
                 output shape satisfied, but no harness session is started and nothing outside \
                 this process is touched. Catches wiring mistakes that validation cannot — an \
                 expression pointing at a node that produces nothing still validates.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "The workflow to simulate." },
                    "input": { "description": "Optional trigger payload; defaults to {}." }
                }),
                &["id"],
            ),
        }),
        json!({
            "name": "workflow_runs",
            "description":
                "The run history for a workflow, newest first — status, steps, and anything a \
                 run is waiting for approval on.",
            "inputSchema": schema(
                json!({ "id": { "type": "string", "description": "The workflow id." } }),
                &["id"],
            ),
        }),
    ]
}

/// A required string argument.
fn arg<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, RpcError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("missing required argument '{name}'")))
}

/// Run a `tools/call`.
pub(super) async fn call(
    store: &Arc<dyn WorkflowStore>,
    params: &Value,
) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing tool name"))?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    let outcome = match name {
        "workflow_list" => ops::list(store).map_err(to_rpc),
        "workflow_get" => ops::get(store, arg(&arguments, "id")?).map_err(to_rpc),
        "workflow_catalog" => {
            ops::catalog(arguments.get("kind").and_then(Value::as_str)).map_err(to_rpc)
        }
        "workflow_create" => {
            let id = arg(&arguments, "id")?;
            ops::create(store, arg(&arguments, "document")?, id).map_err(to_rpc)
        }
        "workflow_apply_ops" => {
            let id = arg(&arguments, "id")?;
            let ops_value = arguments
                .get("ops")
                .ok_or_else(|| RpcError::invalid_params("missing required argument 'ops'"))?;
            ops::apply_ops(store, id, ops_value).map_err(to_rpc)
        }
        "workflow_preview_ops" => {
            let id = arg(&arguments, "id")?;
            let ops_value = arguments
                .get("ops")
                .ok_or_else(|| RpcError::invalid_params("missing required argument 'ops'"))?;
            ops::preview_ops(store, id, ops_value).map_err(to_rpc)
        }
        "workflow_validate" => {
            // Either handle, whichever the author gave: a saved id, or a
            // document they are still writing.
            let handle = match (
                arguments.get("id").and_then(Value::as_str),
                arguments.get("document").and_then(Value::as_str),
            ) {
                (Some(id), _) if !id.trim().is_empty() => GraphHandle::Saved(id),
                (_, Some(document)) if !document.trim().is_empty() => GraphHandle::Inline(document),
                _ => {
                    return Err(RpcError::invalid_params(
                        "workflow_validate: pass either 'id' or 'document'",
                    ))
                }
            };
            Ok(ops::validate(store, &handle))
        }
        "workflow_dry_run" => {
            let id = arg(&arguments, "id")?;
            let input = arguments.get("input").cloned().unwrap_or(json!({}));
            ops::dry_run(store, id, input).await.map_err(to_rpc)
        }
        "workflow_runs" => ops::list_runs(store, arg(&arguments, "id")?).map_err(to_rpc),
        other => {
            return Err(RpcError::invalid_params(format!(
                "unknown tool '{other}'; available: {}",
                TOOL_NAMES.join(", ")
            )))
        }
    };

    // MCP reports a *tool's* failure as content with `isError`, not as a
    // protocol error: the model should read what went wrong and try again,
    // rather than the client treating the call as broken.
    Ok(match outcome {
        Ok(value) => content(&value, false),
        Err(err) => content(&json!({ "error": err.message }), true),
    })
}

/// Wrap a value as an MCP tool result.
fn content(value: &Value, is_error: bool) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        }],
        "isError": is_error,
    })
}

/// A workflow error as an RPC error.
fn to_rpc(err: crate::workflows::WorkflowError) -> RpcError {
    RpcError::invalid_params(err.to_string())
}
