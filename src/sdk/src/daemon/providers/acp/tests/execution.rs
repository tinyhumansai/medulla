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
    let env = super::super::execution::acp_env(&attribution_options(true));
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
    let env = super::super::execution::acp_env(&attribution_options(false));
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

    let env = super::super::execution::acp_env(&options);

    assert!(!env.contains_key(crate::control_socket::MCP_SOCKET_ENV));
    assert!(!env.contains_key(crate::control_socket::MCP_GRANT_ENV));
}
