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

pub use tools::{tool_definitions, ToolMode, TOOL_MODE_ENV, TOOL_NAMES, TOOL_SCOPE_ENV};

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::workflows::WorkflowStore;

/// The MCP protocol version this server speaks.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Versions this server will speak if a client asks for one.
///
/// The specification's negotiation is: answer with the client's version if you
/// support it, otherwise with your own and let the client decide. Answering with
/// a fixed version regardless — which this did — is the one response that tells
/// a client nothing, because it looks identical whether or not its request was
/// understood.
const SUPPORTED_VERSIONS: [&str; 2] = ["2024-11-05", "2025-03-26"];

/// The version to answer `initialize` with, given what the client asked for.
fn negotiate(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|asked| SUPPORTED_VERSIONS.iter().find(|known| **known == asked))
        .copied()
        .unwrap_or(PROTOCOL_VERSION)
}

/// The server's advertised name, as it appears to a model.
pub const SERVER_NAME: &str = "medulla-workflows";

/// Check that a harness session would actually be handed these tools.
///
/// The three ways they silently do not arrive, all of which leave a session that
/// starts fine and can do nothing: workflows turned off, a binary whose own path
/// cannot be resolved, and the `workflows` feature compiled out.
///
/// For a *workflow run* that is survivable — an `agent` node dispatches an
/// instruction and needs no workflow tools to carry it out. For an *authoring*
/// turn it is fatal, because the tools are the whole of what the turn is for:
/// without them the prompt tells a model to call things that are not there, and
/// the operator gets a confident reply and an unchanged graph. So the copilot
/// asks first rather than finding out from the silence.
///
/// # Errors
///
/// Returns the reason the tools would be missing, phrased for an operator who
/// has to fix it.
pub fn preflight(env: &HashMap<String, String>, cwd: &Path) -> Result<(), String> {
    let enabled =
        crate::config::load_config(crate::config::explicit_config_from_env(env), env, cwd)
            .map(|loaded| loaded.config.workflows.enabled)
            .unwrap_or(true);
    if !enabled {
        return Err(
            "workflows are turned off on this host (workflows.enabled = false), so the copilot \
             has no tools to edit a graph with"
                .to_string(),
        );
    }
    if std::env::current_exe().is_err() {
        return Err(
            "cannot determine this binary's own path, so the workflow tool server cannot be \
             started for the harness"
                .to_string(),
        );
    }
    Ok(())
}

/// Handle one JSON-RPC request, returning the response to write back.
///
/// `None` for a notification, which by JSON-RPC definition gets no reply — a
/// server that answered one would confuse the client's request correlation.
pub async fn handle_request(
    store: &Arc<dyn WorkflowStore>,
    config: &crate::config::WorkflowsConfig,
    mode: ToolMode,
    request: &Value,
) -> Option<Value> {
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let id = request.get("id").cloned();
    let params = request.get("params").cloned().unwrap_or(Value::Null);

    // A notification has no id. `initialized` is the one every client sends,
    // and answering it would desynchronise the client's request correlation.
    let id = id?;

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": negotiate(
                params.get("protocolVersion").and_then(Value::as_str)
            ),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
        })),
        "tools/list" => Ok(json!({ "tools": tool_definitions(mode) })),
        "tools/call" => tools::call(store, config, mode, &params).await,
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
    // Loaded once, not per call: this process serves one harness session, and
    // an operator editing config mid-session is not a case worth re-reading a
    // file on every tool call for. Defaults on failure — a server that refused
    // to start because config was unreadable would take the copilot's tools
    // with it, which is worse than answering `workflow_host` conservatively.
    // Respects the parent's `--config`, if it recorded one before spawning this
    // subprocess — see `crate::config::CONFIG_PATH_ENV`. Passing `None`
    // unconditionally here used to make this server re-discover its own
    // config from `cwd`, silently answering with a different policy
    // (`allowCode`, `enabled`, …) than the harness session it serves was
    // actually launched under.
    let config = crate::config::load_config(crate::config::explicit_config_from_env(env), env, cwd)
        .map(|loaded| loaded.config.workflows)
        .unwrap_or_default();
    // The mode the parent asked for. Read once at startup for the same reason
    // config is: this process serves exactly one harness session, and which
    // kind of turn that is was decided before it was spawned.
    let mode = ToolMode::from_env(env);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle_request(&store, &config, mode, &request).await,
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
