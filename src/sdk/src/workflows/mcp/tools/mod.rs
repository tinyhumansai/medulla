//! The tools a harness sees, and the dispatch behind them.
//!
//! Names are prefixed `workflow_` so they cannot collide with the harness's own
//! reserved tool names ([`crate::harness_contract::RESERVED_TOOL_NAMES`]).
//!
//! Descriptions matter more here than anywhere else in this feature: they are
//! the whole of what a model knows before it acts. They live in
//! [`definitions`], apart from the dispatch, because they are long enough that
//! the routing was getting lost among them.

mod definitions;

use std::sync::Arc;

use serde_json::{json, Value};

use crate::workflows::authoring::GraphHandle;
use crate::workflows::{ops, WorkflowStore};

use super::RpcError;

pub use definitions::tool_definitions;

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
