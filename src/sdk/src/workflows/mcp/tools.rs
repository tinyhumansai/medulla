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
///
/// `workflow_run` really does run the thing — harness sessions, scripts, and
/// whatever else the graph describes. It is here because a copilot that can only
/// simulate cannot answer "does this work", and a dry run is structurally blind
/// to exactly the steps most worth checking: a `code` node's script, an
/// `agent` node's real reply. The operator's own switches still apply
/// (`workflows.enabled`, and the workflow's own `enabled`).
///
/// There is still no tool to *cancel* a run. A copilot that started one is
/// awaiting it; the operator cancels from the pane, where they can see it.
pub const TOOL_NAMES: [&str; 14] = [
    "workflow_list",
    "workflow_get",
    "workflow_host",
    "workflow_catalog",
    "workflow_create",
    "workflow_apply_ops",
    "workflow_preview_ops",
    "workflow_validate",
    "workflow_dry_run",
    "workflow_run",
    "workflow_runs",
    "workflow_run_get",
    "workflow_history",
    "workflow_delete",
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
            "name": "workflow_host",
            "description":
                "What this machine will actually permit a workflow to do: the default worker \
                 `agent` nodes dispatch to, the tool slugs and HTTP hosts that are allowed, and \
                 whether `code` nodes may run. Read this before writing a graph that reaches \
                 outside the process — every one of these is enforced at run time, so a graph \
                 that ignores them saves and validates cleanly and then fails the first time it \
                 matters.",
            "inputSchema": schema(json!({}), &[]),
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
            "name": "workflow_run",
            "description":
                "Run a workflow for real: dispatch its `agent` steps to actual coding harnesses, \
                 execute its scripts, and make whatever changes it describes. This is not a \
                 simulation — prefer workflow_dry_run while you are still wiring, and use this \
                 when the operator has asked whether it works, or when a dry run cannot settle \
                 the question (a `code` node's script and an `agent` node's real reply are both \
                 invisible to one). Returns the whole run record: every step, its status, and \
                 anything that resolved to null. Can take minutes.",
            "inputSchema": schema(
                json!({
                    "id": { "type": "string", "description": "The workflow to run." },
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
        json!({
            "name": "workflow_run_get",
            "description":
                "One run in full, by run id: every step, its status, its duration, and the \
                 expressions that resolved to null on the way. Where to start when asked why a \
                 workflow failed — the steps say what actually happened, which is more reliable \
                 than reading the graph and reasoning about what it would do.",
            "inputSchema": schema(
                json!({ "runId": { "type": "string", "description": "The run id." } }),
                &["runId"],
            ),
        }),
        json!({
            "name": "workflow_history",
            "description":
                "The versions of a workflow that have been written over, newest first, each with \
                 the whole graph as it then was. Useful for saying what changed and when — an \
                 edit that broke something is often easier to see next to the version before it. \
                 Restoring one is the operator's own action, not yours.",
            "inputSchema": schema(
                json!({ "id": { "type": "string", "description": "The workflow id." } }),
                &["id"],
            ),
        }),
        json!({
            "name": "workflow_delete",
            "description":
                "Remove a workflow. Only when the operator asked for it in this turn — deleting \
                 something they did not ask about is not a helpful tidy-up. The version is kept \
                 in history, so an operator can undo it, but do not treat that as licence.",
            "inputSchema": schema(
                json!({ "id": { "type": "string", "description": "The workflow to remove." } }),
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
    config: &crate::config::WorkflowsConfig,
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
        "workflow_run" => {
            let id = arg(&arguments, "id")?;
            let input = arguments.get("input").cloned().unwrap_or(json!({}));
            let env: std::collections::HashMap<String, String> = std::env::vars().collect();
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            ops::run(store, config, &env, &cwd, id, input)
                .await
                .map_err(to_rpc)
        }
        "workflow_runs" => ops::list_runs(store, arg(&arguments, "id")?).map_err(to_rpc),
        "workflow_run_get" => ops::get_run(store, arg(&arguments, "runId")?).map_err(to_rpc),
        "workflow_host" => Ok(ops::host_facts(config)),
        "workflow_history" => ops::list_history(store, arg(&arguments, "id")?).map_err(to_rpc),
        "workflow_delete" => ops::delete(store, arg(&arguments, "id")?).map_err(to_rpc),
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
