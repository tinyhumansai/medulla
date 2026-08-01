//! Medulla's Model Context Protocol server, offered to the harnesses it spawns.
//!
//! Medulla drives Claude Code and Codex over ACP as a *client*, so it cannot
//! hand them tools the way a host application does — the tools have to be
//! offered, and MCP is the way an ACP agent accepts them. Attaching this server
//! to a session (`session/new`'s `mcpServers`) is what turns "a harness that can
//! edit files" into one that can build a workflow and delegate to the fleet it
//! is running in.
//!
//! Two tool families are served:
//!
//! * `workflow_*` — authoring, validating, and running saved graphs. These are
//!   the SDK's operations ([`crate::workflows::ops`]), the same ones the
//!   `medulla workflow` subcommand calls, so a model that learns one has learned
//!   both and neither can drift from the other.
//! * `fleet_*` — seeing the fleet and dispatching work into it, proxied to a
//!   running Medulla over the control socket ([`crate::control_socket`]).
//!
//! Which families a session gets is decided by the grant Medulla minted for it,
//! never by anything the session says about itself.
//!
//! # Where the fleet tools exist
//!
//! Only on a host that is *running* a fleet — one whose process bound a control
//! plane ([`crate::control_socket::active`]). In practice that means the
//! operator's own machine. A harness Medulla dispatched to a remote worker is
//! executed by that worker's daemon, which has no hub of its own and so mints no
//! grant; it gets the `workflow_*` family and nothing else.
//!
//! So the same prompt reaches a harness with a different tool surface depending
//! on where the task landed. That is deliberate, and it is why the difference is
//! expressed by *withholding the verbs* rather than by advertising tools that
//! fail: a model on a remote worker is never told about a fleet it cannot reach,
//! so it plans around what it has instead of retrying something structurally
//! unavailable. `fleet_status` exists for the case where a session does hold the
//! family and wants to know the fleet's shape before planning around it.
//!
//! Routing a remote worker's fleet calls back to the orchestrator over the
//! tiny.place bridge would remove the asymmetry, and is deliberately not done
//! here: it needs frames, cross-machine correlation, and abort routing that this
//! surface does not yet have.
//!
//! Only the pieces of MCP a stdio tool server actually needs are implemented —
//! `initialize`, `tools/list`, `tools/call`, and the `notifications/*` a client
//! sends and expects nothing back from. That is the whole protocol surface for a
//! server that exposes tools and no resources or prompts.

pub mod backend;
mod tools;
mod types;

#[cfg(test)]
mod tests;

pub use backend::{FleetBackend, OfflineFleet};
pub use tools::{
    tool_definitions, ToolMode, FLEET_TOOL_NAMES, TOOL_MODE_ENV, TOOL_NAMES, TOOL_SCOPE_ENV,
};
pub use types::McpSession;

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};

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
///
/// Plain `medulla` rather than `medulla-workflows`: it serves the fleet family
/// too now, and a harness reads this in every tool name it sees
/// (`mcp__medulla__fleet_dispatch`).
pub const SERVER_NAME: &str = "medulla";

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
pub async fn handle_request(session: &McpSession, request: &Value) -> Option<Value> {
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
        "tools/list" => Ok(json!({ "tools": tool_definitions(session) })),
        "tools/call" => tools::call(session, &params).await,
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
    // The custom harness presets ride along with the `workflows` section: an
    // author choosing a harness needs to know which presets this machine has,
    // and they live in their own config section rather than that one.
    let policy = crate::config::load_config(crate::config::explicit_config_from_env(env), env, cwd)
        .map(policy_from_loaded)
        .unwrap_or_default();
    // The mode the parent asked for. Read once at startup for the same reason
    // config is: this process serves exactly one harness session, and which
    // kind of turn that is was decided before it was spawned.
    let mode = ToolMode::from_env(env);
    // Connected before the first request rather than lazily: the answer decides
    // which tools to advertise, and `tools/list` is often the very first thing
    // asked. A host with no grant in its environment — a remote worker, or one
    // with fleet tools turned off — gets the offline stand-in and serves the
    // workflow tools alone rather than failing to start.
    let session = McpSession::local(store, policy, mode).with_fleet(backend::from_env(env).await);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle_request(&session, &request).await,
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

/// Build the tool-call policy from a loaded config.
///
/// A custom-harness preset attaches to a fleet `hostId`; one for another
/// machine is not this device's to advertise (`workflow_host`) or execute
/// (`workflow_run`, which reaches [`crate::workflows::local::run_here`] and
/// starts an embedded daemon *on this device*). This is the third local
/// execution path that starts one — the CLI
/// (`commands::workflow::local_custom_harnesses` in the `medulla-tui` crate)
/// and the interactive TUI host (`local_host::options_from_config_with_custom`,
/// also `medulla-tui`) both filter to the effective local `[host]` address the
/// same way; this filters to the same address using
/// [`HostSection::effective_address`](crate::config::HostSection::effective_address),
/// the shared logic both of those now call through.
fn policy_from_loaded(loaded: crate::config::LoadedConfig) -> crate::workflows::ops::HostPolicy {
    let local_host_id = loaded.config.host.effective_address();
    let custom_harness_configs = crate::config::load_layered_custom_harnesses(&loaded.sources)
        .unwrap_or_default()
        .into_iter()
        .filter(|preset| preset.host_id == local_host_id)
        .collect::<Vec<_>>();
    let custom_harnesses = custom_harness_configs
        .iter()
        .map(|preset| preset.id.clone())
        .collect();
    crate::workflows::ops::HostPolicy {
        workflows: loaded.config.workflows,
        custom_harnesses,
        custom_harness_configs,
    }
}

#[cfg(test)]
mod policy_tests {
    use super::policy_from_loaded;
    use crate::config::{CustomHarnessConfig, HostSection, LoadedConfig, TuiConfig};
    use crate::tinyplace::HarnessProvider;

    /// A minimal, valid custom-harness preset for `host_id`.
    fn preset(id: &str, host_id: &str) -> CustomHarnessConfig {
        CustomHarnessConfig {
            id: id.to_string(),
            name: id.to_string(),
            base_harness: HarnessProvider::Claude,
            model: "deepseek/deepseek-chat".to_string(),
            fast_model: None,
            context_window: None,
            host_id: host_id.to_string(),
            default: false,
            api_key_env: "OPENROUTER_API_KEY".to_string(),
            base_url: String::new(),
        }
    }

    /// Writes `presets` as this file's `[[workflows.customHarnesses]]` table
    /// and returns a [`LoadedConfig`] whose `sources` names it, matching what
    /// `crate::config::load_config` would have produced from a real file.
    fn loaded_with_presets(
        root: &std::path::Path,
        host_address: &str,
        presets: &[CustomHarnessConfig],
    ) -> LoadedConfig {
        let path = root.join("medulla.tui.json");
        let rows: Vec<serde_json::Value> = presets
            .iter()
            .map(|preset| serde_json::to_value(preset).unwrap())
            .collect();
        std::fs::write(
            &path,
            serde_json::json!({ "customHarnesses": rows }).to_string(),
        )
        .unwrap();

        let mut config = TuiConfig::default();
        config.host.address = host_address.to_string();

        LoadedConfig {
            config,
            path: path.to_string_lossy().to_string(),
            sources: vec![path.to_string_lossy().to_string()],
        }
    }

    #[test]
    fn presets_for_another_fleet_host_are_not_advertised_or_carried_for_execution() {
        let root = tempfile::tempdir().unwrap();
        let loaded = loaded_with_presets(
            root.path(),
            "this-machine",
            &[
                preset("mine", "this-machine"),
                preset("someone-elses", "other-machine"),
            ],
        );

        let policy = policy_from_loaded(loaded);

        assert_eq!(policy.custom_harnesses, vec!["mine".to_string()]);
        assert_eq!(policy.custom_harness_configs.len(), 1);
        assert_eq!(policy.custom_harness_configs[0].id, "mine");
    }

    #[test]
    fn a_blank_host_address_matches_the_section_default_not_every_preset() {
        let root = tempfile::tempdir().unwrap();
        // The operator never set `[host].address`, so the effective id is the
        // section default — not the blank string a preset might (incorrectly)
        // carry.
        let loaded = loaded_with_presets(
            root.path(),
            "",
            &[preset("mine", &HostSection::default().address)],
        );

        let policy = policy_from_loaded(loaded);

        assert_eq!(policy.custom_harnesses, vec!["mine".to_string()]);
    }
}
