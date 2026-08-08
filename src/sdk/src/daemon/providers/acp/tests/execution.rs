//! ACP execution, session, grant, and environment regressions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::daemon::providers::{Abort, RunTaskOptions};
use crate::protocol::HarnessProvider;

#[cfg(unix)]
#[tokio::test]
async fn a_new_acp_session_is_reported_before_the_task_completes() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let agent = dir.path().join("fake-opencode");
    std::fs::write(
        &agent,
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([^,}]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1}}\n' "$id" ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"acp-session-1"}}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id" ;;
  esac
done
"#,
    )
    .unwrap();
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();

    let reported = Arc::new(Mutex::new(Vec::new()));
    let mut options = attribution_options(false);
    options.provider = HarnessProvider::Opencode;
    options.cwd = dir.path().to_string_lossy().into_owned();
    options.env.insert(
        "MEDULLA_OPENCODE_BIN".to_string(),
        agent.to_string_lossy().into_owned(),
    );
    options.on_session = Some({
        let reported = reported.clone();
        Box::new(move |session_id| reported.lock().unwrap().push(session_id))
    });

    let result = super::super::execution::run_acp_task(options)
        .await
        .unwrap();

    assert_eq!(reported.lock().unwrap().as_slice(), ["acp-session-1"]);
    assert_eq!(result.session_id.as_deref(), Some("acp-session-1"));
}

#[cfg(all(feature = "workflows", unix))]
struct NoFleet;

#[cfg(all(feature = "workflows", unix))]
#[async_trait::async_trait]
impl crate::control_socket::FleetOps for NoFleet {
    fn workers(&self) -> Option<Vec<crate::control_socket::FleetWorker>> {
        Some(Vec::new())
    }

    fn default_worker(&self) -> Option<String> {
        None
    }

    async fn dispatch(
        &self,
        _request: crate::hub::TaskRequest,
        _status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<crate::hub::TaskOutcome, crate::hub::RunError> {
        unreachable!("grant exchange never dispatches")
    }

    fn abort(&self, _abort_id: &str) -> bool {
        false
    }
}

#[cfg(all(feature = "workflows", unix))]
#[tokio::test]
async fn a_parent_handoff_is_exchanged_before_the_mcp_server_is_attached() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("control.sock");
    let grants = crate::control_socket::GrantRegistry::new();
    let parent_token = grants.mint(crate::control_socket::Grant::new("parent", 1, 3));
    let ops: Arc<dyn crate::control_socket::FleetOps> = Arc::new(NoFleet);
    let _server = crate::control_socket::ControlServer::bind(&path, ops, grants)
        .await
        .unwrap();
    let env = HashMap::from([
        (
            crate::control_socket::MCP_PARENT_SOCKET_ENV.to_string(),
            path.to_string_lossy().into_owned(),
        ),
        (
            crate::control_socket::MCP_PARENT_GRANT_ENV.to_string(),
            parent_token.clone(),
        ),
    ]);

    let servers = super::super::execution::medulla_mcp_servers(None, "child", &env).await;
    let agent_client_protocol::schema::v1::McpServer::Stdio(server) = &servers[0] else {
        panic!("Medulla MCP server must use stdio");
    };
    let child_token = server
        .env
        .iter()
        .find(|var| var.name == crate::control_socket::MCP_GRANT_ENV)
        .map(|var| var.value.clone())
        .expect("child grant attached");
    let child = crate::control_socket::ControlClient::connect(&path, &child_token)
        .await
        .unwrap();

    assert_ne!(child_token, parent_token);
    assert_eq!(child.hello().depth, 2);
    assert_eq!(child.hello().max_depth, 3);
}

#[cfg(feature = "workflows")]
#[test]
fn session_grants_read_depth_from_the_task_environment() {
    let env = HashMap::from([(
        crate::control_socket::FLEET_DEPTH_ENV.to_string(),
        "2".to_string(),
    )]);

    let grant = crate::mcp::attach::session_grant("nested", &env, Some("propose:demo"), true, 3, 5);

    assert_eq!(grant.depth, 2);
    assert_eq!(grant.max_depth, 3);
    assert_eq!(grant.max_in_flight, 5);
    assert_eq!(grant.tool_mode.as_deref(), Some("propose:demo"));
}

#[cfg(feature = "workflows")]
#[test]
fn disabling_workflows_keeps_the_fleet_family_on_the_session_grant() {
    let grant = crate::mcp::attach::session_grant("fleet-only", &HashMap::new(), None, false, 2, 4);

    assert!(!grant.families.workflows);
    assert!(grant.families.fleet);
}
// ---------------------------------------------------------------------------
// Attribution reaches the ACP spawn path
// ---------------------------------------------------------------------------

/// A `RunTaskOptions` carrying `attribution`, with everything else inert.
fn attribution_options(attribution: bool) -> RunTaskOptions {
    RunTaskOptions {
        hooks: crate::harness_hooks::HooksConfig::default(),
        transport: Default::default(),
        conversation: String::new(),
        session_class: crate::sessions::SessionClass::Bounded,
        resume_session_id: None,
        workspace_context: Default::default(),
        provider: HarnessProvider::Claude,
        prompt: String::new(),
        cwd: ".".to_string(),
        env: HashMap::new(),
        timeout_ms: 1_000,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        abort: Abort::new(),
        router: None,
        attribution,
        on_event: None,
        on_stdin: None,
        on_session: None,
        on_workspace_context: None,
    }
}

/// `run_provider_task` dispatches to ACP *before* the spawn seam that applies
/// attribution for direct runs, so the ACP agent env must carry it itself —
/// otherwise every ACP-backed commit is unattributed.
#[cfg(unix)]
#[test]
fn agent_env_carries_attribution() {
    let env = super::super::execution::acp_env(&attribution_options(true)).unwrap();
    assert!(
        env.contains_key("MEDULLA_ATTRIBUTION"),
        "ACP agent env must carry the attribution trailer"
    );
    assert_eq!(
        env.get("GIT_CONFIG_KEY_0").map(String::as_str),
        Some("core.hooksPath"),
        "ACP agent env must activate the hook directory"
    );
}

/// Turning attribution off leaves the ACP env untouched.
#[test]
fn agent_env_omits_attribution_when_off() {
    let env = super::super::execution::acp_env(&attribution_options(false)).unwrap();
    assert!(!env.contains_key("MEDULLA_ATTRIBUTION"));
    assert!(!env.contains_key("GIT_CONFIG_KEY_0"));
}

#[test]
fn agent_env_strips_inherited_fleet_capabilities() {
    let mut options = attribution_options(false);
    options.env.insert(
        crate::control_socket::MCP_SOCKET_ENV.to_string(),
        "/tmp/another-session.sock".to_string(),
    );
    options.env.insert(
        crate::control_socket::MCP_GRANT_ENV.to_string(),
        "another-session-token".to_string(),
    );

    let env = super::super::execution::acp_env(&options).unwrap();

    assert!(!env.contains_key(crate::control_socket::MCP_SOCKET_ENV));
    assert!(!env.contains_key(crate::control_socket::MCP_GRANT_ENV));
}

#[test]
fn agent_env_strips_the_embedded_core_workspace() {
    let mut options = attribution_options(false);
    options.env.insert(
        "OPENHUMAN_WORKSPACE".to_string(),
        "/live-core-workspace".to_string(),
    );

    let env = super::super::execution::acp_env(&options).unwrap();

    assert!(!env.contains_key("OPENHUMAN_WORKSPACE"));
}

/// ACP's client library overlays configured values on an inherited process
/// environment, so the final command must remove the workspace as well as its
/// configured map omitting it.
#[cfg(unix)]
#[test]
fn agent_command_removes_the_embedded_core_workspace() {
    let agent = super::super::execution::agent_for(&attribution_options(false)).unwrap();
    let config = agent.config();

    assert_eq!(config.command().to_string_lossy(), "env");
    assert_eq!(
        config.arguments(),
        [
            "-u",
            "OPENHUMAN_WORKSPACE",
            "npx",
            "-y",
            "@agentclientprotocol/claude-agent-acp@latest"
        ]
    );
}

/// A routed Codex preset reaching ACP dispatch must carry its model and its
/// provider overrides **in the environment**.
///
/// Two regressions in one, and both were silent. First ACP built a bare
/// `codex-acp`, so the preset's model was dropped and `codex_overrides` never
/// ran at all. Then the overrides were put on the argv — which `codex-acp`
/// parses only for its `login` and `cli` subcommands and ignores completely in
/// server mode, where it reads `CODEX_CONFIG` and `MODEL_PROVIDER` from the
/// environment instead. Either way Codex served the operator's own default model
/// from their own account while the routed endpoint sat unused beside it: the
/// run looked healthy and not one request reached the configured provider.
///
/// Hence the assertion is on `environment()`, not on argv. An argv-only check is
/// exactly what let the second regression through.
#[cfg(unix)]
#[test]
fn codex_acp_command_carries_the_routed_model_and_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    std::fs::create_dir_all(&codex_home).unwrap();
    // Routed runs derive a provider-safe catalog from Codex's local template.
    // Seed the smallest usable template so this argv regression stays offline
    // and independent of the developer or CI runner's Codex home.
    std::fs::write(
        codex_home.join("models_cache.json"),
        r#"{"models":[{"slug":"gpt-5.4","priority":1,"context_window":200000}]}"#,
    )
    .unwrap();
    let mut options = attribution_options(false);
    options.provider = HarnessProvider::Codex;
    options.model = Some("deepseek/deepseek-v4-flash-0731".to_string());
    options.env.insert(
        crate::codex_overrides::OVERRIDES_ENV.to_string(),
        "1".to_string(),
    );
    options.env.insert(
        "OPENAI_BASE_URL".to_string(),
        "http://127.0.0.1:36277/openai".to_string(),
    );
    options.env.insert(
        "CODEX_HOME".to_string(),
        codex_home.to_string_lossy().into_owned(),
    );
    options.env.insert(
        "MEDULLA_HOME".to_string(),
        dir.path().join("medulla").to_string_lossy().into_owned(),
    );

    let agent = super::super::execution::agent_for(&options).unwrap();
    let args: Vec<String> = agent
        .config()
        .arguments()
        .iter()
        .map(ToString::to_string)
        .collect();

    let model_at = args
        .iter()
        .position(|argument| argument == "-m")
        .expect("routed Codex ACP argv must select the preset's model");
    assert_eq!(
        args.get(model_at + 1).map(String::as_str),
        Some("deepseek/deepseek-v4-flash-0731")
    );

    let environment = agent.config().environment();
    assert_eq!(
        environment
            .get(crate::codex_overrides::MODEL_PROVIDER_ENV)
            .map(String::as_str),
        Some("medulla"),
        "codex-acp selects its provider from MODEL_PROVIDER: {environment:?}"
    );
    let config: serde_json::Value = serde_json::from_str(
        environment
            .get(crate::codex_overrides::CONFIG_ENV)
            .expect("routed Codex ACP must carry CODEX_CONFIG"),
    )
    .expect("CODEX_CONFIG must be a JSON document");
    assert_eq!(config["model"], "deepseek/deepseek-v4-flash-0731");
    assert_eq!(config["model_provider"], "medulla");
    assert_eq!(
        config["model_providers"]["medulla"]["base_url"], "http://127.0.0.1:36277/openai",
        "the routed endpoint is the whole point of the block"
    );
    assert_eq!(
        config["model_providers"]["medulla"]["env_key"], "OPENAI_API_KEY",
        "the key is resolved by name from the environment, never inlined"
    );
    assert_eq!(
        config["preferred_auth_method"], "apikey",
        "without this a signed-in ChatGPT account outranks the routed key"
    );
    assert!(
        config["model_catalog_json"].is_string(),
        "the derived catalog governs the tool shapes sent to the provider: {config}"
    );
}

/// An unrouted Codex ACP run is left exactly as it was: no endpoint means no
/// provider block, and overriding `model_provider` would move a run that never
/// asked to be routed off the operator's own account.
#[cfg(unix)]
#[test]
fn codex_acp_command_stays_unrouted_without_an_endpoint() {
    let mut options = attribution_options(false);
    options.provider = HarnessProvider::Codex;

    let agent = super::super::execution::agent_for(&options).unwrap();
    let args: Vec<String> = agent
        .config()
        .arguments()
        .iter()
        .map(ToString::to_string)
        .collect();

    assert!(
        !args.iter().any(|argument| argument == "-m"),
        "no model was configured, so none may be selected: {args:?}"
    );
    let environment = agent.config().environment();
    assert!(
        !environment.contains_key(crate::codex_overrides::CONFIG_ENV)
            && !environment.contains_key(crate::codex_overrides::MODEL_PROVIDER_ENV),
        "an unrouted run must keep Codex's own provider: {environment:?}"
    );
}

/// A routed ACP run must fail before launching when its derived catalog cannot
/// be built; starting without it silently selects the wrong account or emits
/// unsupported tool shapes at the routed provider.
#[test]
fn routed_codex_acp_rejects_missing_override_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let mut options = attribution_options(false);
    options.provider = HarnessProvider::Codex;
    options.model = Some("deepseek/deepseek-v4-flash-0731".to_string());
    options.env.insert(
        crate::codex_overrides::OVERRIDES_ENV.to_string(),
        "1".to_string(),
    );
    options.env.insert(
        "OPENAI_BASE_URL".to_string(),
        "http://127.0.0.1:36277/openai".to_string(),
    );
    options.env.insert(
        "CODEX_HOME".to_string(),
        dir.path().join("codex").to_string_lossy().into_owned(),
    );

    let error = super::super::execution::agent_for(&options).unwrap_err();

    assert!(error.contains("models_cache.json"), "{error}");
}

/// ACP must reject a routed provider whose configured key source is absent,
/// before its provider overrides can select that endpoint.
#[test]
fn routed_acp_rejects_missing_router_api_key() {
    let mut options = attribution_options(false);
    options.provider = HarnessProvider::Codex;
    options.router = Some(crate::config::RouterConfig {
        base_url: Some("https://gateway.example/v1".to_string()),
        api_key_env: Some("MISSING_ROUTER_KEY".to_string()),
        ..Default::default()
    });

    let error = super::super::execution::agent_for(&options).unwrap_err();

    assert_eq!(
        error,
        "router API key env var `MISSING_ROUTER_KEY` is not set; export it or remove apiKeyEnv from [router]"
    );
}

/// Windows command wrapping must preserve the TOML quotes in Codex's `-c`
/// arguments; Codex parses the text after `=` as TOML rather than a shell word.
#[cfg(windows)]
#[test]
fn windows_cmd_quoting_preserves_embedded_toml_quotes() {
    assert_eq!(
        super::super::execution::quote_windows_cmd_arg("model_provider=\"medulla\""),
        "\"model_provider=\"\"medulla\"\"\""
    );
}
