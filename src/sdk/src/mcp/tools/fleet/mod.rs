//! The `fleet_*` tools: seeing the fleet and dispatching work into it.
//!
//! Every one of these is a thin translation onto a control-plane op
//! ([`crate::control_socket`]). The decisions — who may dispatch, how deep, how
//! many at once, whose tasks are whose — all live server-side, keyed by the
//! grant this session presented. Nothing here is trusted to enforce them; this
//! module's job is to ask well and to render the answer in terms a model can act
//! on.
//!
//! Names are prefixed `fleet_` so they collide neither with the `workflow_`
//! family nor with the harness's own reserved verbs
//! ([`crate::harness_contract::RESERVED_TOOL_NAMES`], which claims the bare
//! `task_*` names).

mod definitions;

#[cfg(test)]
mod tests;

pub(super) use definitions::definitions;

use serde_json::{json, Value};

use crate::control_socket::ControlError;

use super::super::{McpSession, RpcError};

/// Every fleet tool, whether or not a given session is served it.
pub const FLEET_TOOL_NAMES: [&str; 6] = [
    "fleet_status",
    "fleet_workers",
    "fleet_dispatch",
    "fleet_result",
    "fleet_abort",
    "fleet_tasks",
];

/// The default `fleet_result` wait, in seconds.
///
/// Comfortably inside the per-call timeout an MCP client imposes, so a poll
/// answers rather than being cut off — the caller loops instead of losing the
/// handle.
const DEFAULT_WAIT_SECONDS: u64 = 25;

/// Run one `fleet_*` tool.
///
/// # Errors
///
/// A missing required argument is a real JSON-RPC error, matching the
/// `workflow_*` family. Everything else — no fleet, a refused dispatch, an
/// unknown handle — comes back as `isError` content, because it is something the
/// model should read and act on rather than a broken call.
pub(super) async fn call(
    session: &McpSession,
    name: &str,
    arguments: &Value,
) -> Result<Value, RpcError> {
    let outcome = match name {
        "fleet_status" => status(session).await,
        "fleet_workers" => op(session, "worker.list", json!({})).await,
        "fleet_tasks" => op(session, "task.list", json!({})).await,
        "fleet_dispatch" => {
            // Required, and a real protocol error when absent: a dispatch with
            // no brief is not a request that can be partially honoured.
            let instruction = super::dispatch::arg(arguments, "instruction")?;
            op(
                session,
                "task.dispatch",
                dispatch_params(instruction, arguments),
            )
            .await
        }
        "fleet_result" => {
            let task_id = super::dispatch::arg(arguments, "taskId")?;
            let wait = arguments
                .get("waitSeconds")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_WAIT_SECONDS);
            op(
                session,
                "task.get",
                json!({ "taskId": task_id, "waitSeconds": wait }),
            )
            .await
        }
        "fleet_abort" => {
            let task_id = super::dispatch::arg(arguments, "taskId")?;
            op(session, "task.abort", json!({ "taskId": task_id })).await
        }
        other => {
            return Err(RpcError::invalid_params(format!(
                "unknown fleet tool '{other}'"
            )))
        }
    };

    Ok(match outcome {
        Ok(value) => super::dispatch::content(&value, false),
        Err(message) => super::dispatch::content(&json!({ "error": message }), true),
    })
}

/// What `fleet_status` reports, combining stable grant facts with live readiness.
async fn status(session: &McpSession) -> Result<Value, String> {
    match session.fleet.hello().cloned() {
        Some(hello) => {
            let hub_ready = match session.fleet.call("worker.list", json!({})).await {
                Ok(_) => true,
                Err(ControlError::Refused(failure))
                    if failure.kind == crate::control_socket::ErrorKind::HubNotReady =>
                {
                    false
                }
                Err(error) => return Err(describe_failure(error)),
            };
            Ok(json!({
                "connected": true,
                "version": hello.version,
                "hubReady": hub_ready,
                "depth": hello.depth,
                "maxDepth": hello.max_depth,
                "maxInFlight": hello.max_in_flight,
                "mayDispatch": hello.may_dispatch(),
            }))
        }
        None => Ok(json!({
            "connected": false,
            "mayDispatch": false,
            "detail":
                "no Medulla fleet is reachable from this session — do the work here rather \
                 than waiting for one",
        })),
    }
}

/// The dispatch request, with the optional hints the caller supplied.
///
/// Only the fields a model has any business choosing. The task and abort ids,
/// the cycle id, the tool mode, and session continuity are all minted by the
/// control plane: they are capability handles keyed into registries shared by
/// every dispatch, not names.
fn dispatch_params(instruction: &str, arguments: &Value) -> Value {
    let mut params = json!({ "instruction": instruction });
    let object = params.as_object_mut().expect("a json object");
    for hint in ["worker", "harness", "model", "workflow"] {
        if let Some(value) = arguments
            .get(hint)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            object.insert(hint.to_string(), json!(value));
        }
    }
    params
}

/// Issue one control-plane op, rendering any failure as prose for a model.
async fn op(session: &McpSession, op: &str, params: Value) -> Result<Value, String> {
    session
        .fleet
        .call(op, params)
        .await
        .map_err(describe_failure)
}

/// Turn a control-plane failure into something a model can act on.
///
/// The distinction that has to survive: whether trying again is worth anything.
/// A refusal carries that on the wire; a lost connection carries the warning
/// that the request's fate is genuinely unknown.
fn describe_failure(error: ControlError) -> String {
    match error {
        ControlError::NoInstance => {
            "no Medulla fleet is reachable from this session, so there is nothing to dispatch \
             to — do the work here"
                .to_string()
        }
        ControlError::Refused(failure) => {
            if failure.kind.is_retryable() {
                format!("{} (worth trying again)", failure.message)
            } else {
                failure.message
            }
        }
        ControlError::Disconnected(reason) => reason,
        ControlError::Transport(reason) => reason,
    }
}
