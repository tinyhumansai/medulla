//! Tool-call dispatch and the shared JSON helpers behind tool definitions.
//!
//! Names are prefixed `workflow_` so they cannot collide with the harness's own
//! reserved tool names ([`crate::harness_contract::RESERVED_TOOL_NAMES`]).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::workflows::authoring::GraphHandle;
use crate::workflows::{ops, WorkflowStore};

use super::super::RpcError;
use super::evolve::ToolMode;

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
pub const TOOL_NAMES: [&str; 18] = [
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
    "workflow_notes",
    "workflow_note_add",
    "workflow_proposals",
    "workflow_propose",
];

/// A JSON Schema object with the given properties and required keys.
pub(super) fn schema(properties: Value, required: &[&str]) -> Value {
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
pub(crate) async fn call(
    store: &Arc<dyn WorkflowStore>,
    config: &crate::config::WorkflowsConfig,
    mode: ToolMode,
    params: &Value,
) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing tool name"))?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    // Checked before the match rather than inside each arm: a withheld tool is
    // withheld whatever its arguments are, and a guard per arm is a guard that
    // will eventually be forgotten on a new one.
    if !mode.allows(name) {
        return Ok(content(&json!({ "error": mode.refusal(name) }), true));
    }
    if let Some(error) = scope_error(mode, name, &arguments) {
        return Ok(content(&json!({ "error": error }), true));
    }

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
        "workflow_notes" => ops::notes(store, arg(&arguments, "id")?).map_err(to_rpc),
        "workflow_note_add" => ops::add_note(
            store,
            arg(&arguments, "id")?,
            arg(&arguments, "kind")?,
            arg(&arguments, "text")?,
            string_list(&arguments, "runIds"),
            // Anything reaching this server over a tool call is a model. An
            // operator's own note comes in through the CLI or the TUI, and the
            // two are weighted differently in a brief.
            crate::workflows::NoteSource::Agent { model: None },
            string_list(&arguments, "supersedes"),
        )
        .map_err(to_rpc),
        "workflow_proposals" => ops::proposals(store, arg(&arguments, "id")?).map_err(to_rpc),
        "workflow_propose" => {
            let id = arg(&arguments, "id")?;
            let rationale = arg(&arguments, "rationale")?;
            let ops_value = arguments
                .get("ops")
                .ok_or_else(|| RpcError::invalid_params("missing required argument 'ops'"))?;
            ops::propose(
                store,
                id,
                rationale,
                ops_value,
                string_list(&arguments, "runIds"),
                string_list(&arguments, "noteIds"),
            )
            .await
            .map_err(to_rpc)
        }
        other => {
            return Err(RpcError::invalid_params(format!(
                "unknown tool '{other}'; available: {}",
                served(mode).join(", ")
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

/// Refuse evolution writes that target a workflow other than the reviewed one.
fn scope_error(mode: ToolMode, name: &str, arguments: &Value) -> Option<String> {
    let scope = std::env::var(super::evolve::TOOL_SCOPE_ENV).ok();
    scope_error_for(mode, name, arguments, scope.as_deref())
}

pub(crate) fn scope_error_for(
    mode: ToolMode,
    name: &str,
    arguments: &Value,
    scope: Option<&str>,
) -> Option<String> {
    if mode != ToolMode::Propose || !matches!(name, "workflow_note_add" | "workflow_propose") {
        return None;
    }
    let scope = scope?;
    let requested = arguments.get("id").and_then(Value::as_str).unwrap_or("");
    (requested != scope).then(|| {
        format!(
            "'{name}' is scoped to workflow '{scope}' for this review; \
             writing workflow '{requested}' is not allowed"
        )
    })
}

/// The tool names this mode serves.
pub fn served(mode: ToolMode) -> Vec<&'static str> {
    TOOL_NAMES
        .into_iter()
        .filter(|name| mode.allows(name))
        .collect()
}

/// An optional array-of-strings argument, absent or malformed alike as empty.
///
/// Forgiving on purpose: a model that sends a bare string instead of an array
/// has still told us something, and failing the whole call over the shape of an
/// optional provenance field costs a round trip to learn nothing.
fn string_list(arguments: &Value, name: &str) -> Vec<String> {
    match arguments.get(name) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(single)) => vec![single.clone()],
        _ => Vec::new(),
    }
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
