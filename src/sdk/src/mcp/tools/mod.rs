//! Shared MCP tool-family routing and JSON helpers.
//!
//! Workflow operations are owned by [`crate::workflows::mcp`]; fleet proxying
//! stays here beside the shared protocol server. This module only aggregates
//! their definitions, enforces the session grant, and routes calls across that
//! boundary.

pub mod fleet;

#[cfg(test)]
pub(crate) use crate::workflows::mcp::scope_error_for;
pub use crate::workflows::mcp::{ToolMode, TOOL_MODE_ENV, TOOL_NAMES, TOOL_SCOPE_ENV};
pub use fleet::FLEET_TOOL_NAMES;

use serde_json::{json, Value};

use super::{McpSession, RpcError};

/// The tool definitions this session is served.
///
/// Filtering is centralized at the shared server boundary so every family is
/// withheld consistently when the session grant or workflow mode excludes it.
pub fn tool_definitions(session: &McpSession) -> Vec<Value> {
    let mut tools = crate::workflows::mcp::definitions();
    // A fleet grant is fixed when Medulla spawns the harness. With no live
    // backend there is no later state change that justifies advertising even
    // `fleet_status` against thin air.
    if let Some(hello) = session.fleet.hello() {
        tools.extend(fleet::definitions(schema, hello.may_dispatch()));
    }
    tools
        .into_iter()
        .filter(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| session.mode.allows(name) && session.families.allows(name))
        })
        .collect()
}

/// Route one `tools/call` request to its owning family.
pub(super) async fn call(session: &McpSession, params: &Value) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("missing tool name"))?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    // A tool excluded by the grant or mode is refused before family dispatch,
    // whatever arguments the caller supplied.
    if !session.families.allows(name) {
        return Ok(content(
            &json!({
                "error": format!(
                    "'{name}' is not available to this session — the operator did not grant \
                     it. Work with the tools you were offered."
                )
            }),
            true,
        ));
    }
    if !session.mode.allows(name) {
        return Ok(content(
            &json!({ "error": session.mode.refusal(name) }),
            true,
        ));
    }

    if name.starts_with("fleet_") {
        fleet::call(session, name, &arguments).await
    } else {
        crate::workflows::mcp::call(session, name, arguments).await
    }
}

/// Build a JSON Schema object with the given properties and required keys.
pub(crate) fn schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

/// Read a required, non-empty string argument.
pub(crate) fn arg<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, RpcError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("missing required argument '{name}'")))
}

/// Wrap a value as an MCP tool result.
pub(crate) fn content(value: &Value, is_error: bool) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        }],
        "isError": is_error,
    })
}
