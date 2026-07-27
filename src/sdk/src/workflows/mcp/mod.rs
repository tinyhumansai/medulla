//! A Model Context Protocol server exposing the workflow operations.
//!
//! Medulla drives Claude Code and Codex over ACP as a *client*, so it cannot
//! hand them tools the way a host application does — the tools have to be
//! offered, and MCP is the way an ACP agent accepts them. Attaching this server
//! to a session (`session/new`'s `mcpServers`) is what turns "a harness that can
//! edit files" into "a harness that can build a workflow".
//!
//! The operations are the SDK's ([`crate::workflows::ops`]), the same ones the
//! `medulla workflow` subcommand calls, so a model that learns one has learned
//! both and neither can drift from the other.
//!
//! Only the pieces of MCP a stdio tool server actually needs are implemented —
//! `initialize`, `tools/list`, `tools/call`, and the `notifications/*` a client
//! sends and expects nothing back from. That is the whole protocol surface for a
//! server that exposes tools and no resources or prompts.

mod tools;

#[cfg(test)]
mod tests;

pub use tools::{tool_definitions, TOOL_NAMES};

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::workflows::WorkflowStore;

/// The MCP protocol version this server speaks.
///
/// Echoed back on `initialize`; a client asking for a different one still gets
/// this, which is what the specification says to do rather than failing.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// The server's advertised name, as it appears to a model.
pub const SERVER_NAME: &str = "medulla-workflows";

/// Handle one JSON-RPC request, returning the response to write back.
///
/// `None` for a notification, which by JSON-RPC definition gets no reply — a
/// server that answered one would confuse the client's request correlation.
pub async fn handle_request(store: &Arc<dyn WorkflowStore>, request: &Value) -> Option<Value> {
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let id = request.get("id").cloned();
    let params = request.get("params").cloned().unwrap_or(Value::Null);

    // A notification has no id. `initialized` is the one every client sends,
    // and answering it would desynchronise the client's request correlation.
    let id = id?;

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        })),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => tools::call(store, &params).await,
        // Answered rather than errored: some clients probe for these before
        // deciding what the server offers, and an error reads as a broken
        // server rather than an empty list.
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        "ping" => Ok(json!({})),
        other => Err(RpcError::method_not_found(other)),
    };

    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": error.code, "message": error.message },
        }),
    })
}

/// A JSON-RPC error.
pub(crate) struct RpcError {
    /// The JSON-RPC error code.
    pub code: i64,
    /// The human-readable message; for a tool this is what the model reads.
    pub message: String,
}

impl RpcError {
    /// The method is not one this server implements.
    fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
        }
    }

    /// The request was understood but its arguments were not usable.
    pub(crate) fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }
}

/// Serve MCP over stdin/stdout until the client closes the stream.
///
/// One JSON object per line, which is the stdio transport MCP defines and the
/// one every ACP agent supports.
///
/// # Errors
///
/// Returns an error only when stdin or stdout fails; a malformed request is
/// answered with a JSON-RPC error and the loop continues, because a client that
/// sent one bad frame is still a client.
pub async fn serve_stdio(env: &HashMap<String, String>, cwd: &Path) -> Result<(), std::io::Error> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let store = crate::workflows::ops::discover_store(env, cwd);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle_request(&store, &request).await,
            Err(err) => Some(json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": { "code": -32700, "message": format!("parse error: {err}") },
            })),
        };
        if let Some(response) = response {
            stdout.write_all(format!("{response}\n").as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}
