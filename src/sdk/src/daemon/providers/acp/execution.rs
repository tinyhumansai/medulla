//! ACP client setup and one-shot harness task execution.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, Error as AcpError, InitializeRequest, LoadSessionRequest,
    NewSessionRequest, PromptRequest, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo};

use crate::protocol::HarnessProvider;

use super::super::types::{RunTaskOptions, RunTaskResult};
use super::types::FoldState;

/// The MCP servers offered to every ACP session.
///
/// Medulla's own tool server, run as this same binary in a subprocess. Empty
/// when the binary's path cannot be determined — a session without the tools is
/// a session that can still do its actual job, which is better than failing to
/// start one.
///
/// When this process is serving a control plane, a grant is minted here for
/// *this* session and handed to that one child. That is what makes the fleet
/// tools exclusive: the token is the authority, it is never written to disk, and
/// a process Medulla did not spawn has no way to obtain one.
///
/// A process with no local control plane can still receive a verified parent
/// handoff from a workflow MCP subprocess. It exchanges that capability for a
/// child grant at the next depth immediately before spawning this session.
/// Ordinary remote workers receive neither source and expose no fleet verbs.
pub(super) async fn medulla_mcp_servers(
    tool_mode: Option<&str>,
    session: &str,
    task_env: &HashMap<String, String>,
) -> Vec<agent_client_protocol::schema::v1::McpServer> {
    #[cfg(not(feature = "workflows"))]
    {
        let _ = (tool_mode, session, task_env);
        Vec::new()
    }
    #[cfg(feature = "workflows")]
    {
        use agent_client_protocol::schema::v1::{McpServer, McpServerStdio};
        // An operator who turned workflows off should not have harnesses handed
        // tools that would be refused.
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        // Respects the parent's `--config`, if one was recorded — see
        // `crate::config::CONFIG_PATH_ENV`. Rediscovering here regardless of
        // that would let a policy the operator explicitly chose (say,
        // `allowCode = false`) be silently overridden by whatever config this
        // subprocess's own `cwd` happens to discover.
        let workflows_enabled = crate::config::load_config(
            crate::config::explicit_config_from_env(task_env),
            task_env,
            &cwd,
        )
        .map(|loaded| loaded.config.workflows.enabled)
        .unwrap_or(true);
        let active_plane = crate::control_socket::active();
        let parent_grant = crate::control_socket::parent_grant_from_env(task_env);
        // With neither family available there is no server worth attaching.
        // A host running a fleet still attaches it when workflow authoring is
        // disabled; the grant below withholds only the workflow family.
        if !workflows_enabled && active_plane.is_none() && parent_grant.is_none() {
            return Vec::new();
        }
        let Ok(binary) = std::env::current_exe() else {
            return Vec::new();
        };
        // The subprocess inherits this process's environment, which is what
        // carries MEDULLA_HOME — so the harness edits the same workflow store
        // the operator sees.
        //
        // The tool mode is passed *explicitly* rather than inherited, because
        // it is the one setting that differs per task: this daemon serves
        // authoring turns and review turns from one process, so an inherited
        // value could only ever be right for one of them. A review turn that
        // silently got the full surface could rewrite the graph it was asked
        // only to review, which is exactly what the mode exists to prevent.
        let mut server =
            McpServerStdio::new(crate::mcp::SERVER_NAME, binary).args(vec!["mcp".to_string()]);
        // The fleet grant, when this process has a control plane to grant
        // against. Pushed explicitly rather than inherited for the same reason
        // the tool mode is: it is minted per session, and an inherited one would
        // hand a second harness the first one's capability.
        let fleet_grant = if let Some(plane) = active_plane {
            // The depth this task was dispatched at, written into the harness
            // environment by the daemon from the task frame. Read here and
            // recorded in the grant, so every later check consults the grant
            // rather than an environment the harness itself could rewrite.
            let grant = session_grant(
                session,
                task_env,
                tool_mode,
                workflows_enabled,
                plane.max_depth,
                plane.max_in_flight,
            );
            Some((plane.socket.clone(), plane.grants.mint(grant)))
        } else if let Some((socket, token)) = parent_grant {
            exchange_parent_grant(socket, token).await
        } else {
            None
        };
        if let Some((socket, token)) = fleet_grant {
            server
                .env
                .push(agent_client_protocol::schema::v1::EnvVariable::new(
                    crate::control_socket::MCP_SOCKET_ENV,
                    socket.to_string_lossy().as_ref(),
                ));
            server
                .env
                .push(agent_client_protocol::schema::v1::EnvVariable::new(
                    crate::control_socket::MCP_GRANT_ENV,
                    token,
                ));
        }
        if let Some(mode) = tool_mode {
            let (mode, scope) = mode
                .split_once(':')
                .map_or((mode, None), |(mode, scope)| (mode, Some(scope)));
            server
                .env
                .push(agent_client_protocol::schema::v1::EnvVariable::new(
                    crate::mcp::TOOL_MODE_ENV,
                    mode,
                ));
            if let Some(scope) = scope {
                server
                    .env
                    .push(agent_client_protocol::schema::v1::EnvVariable::new(
                        crate::mcp::TOOL_SCOPE_ENV,
                        scope,
                    ));
            }
        }
        vec![McpServer::Stdio(server)]
    }
}

/// Exchange a verified parent handoff for the child-only grant attached to ACP.
///
/// The control transport is currently Unix-only. Other targets fail closed and
/// omit fleet tools instead of exposing an unverified capability path.
#[cfg(all(feature = "workflows", unix))]
async fn exchange_parent_grant(socket: PathBuf, token: String) -> Option<(PathBuf, String)> {
    let mut client = crate::control_socket::ControlClient::connect(&socket, &token)
        .await
        .ok()?;
    client
        .call_until(
            "grant.child",
            serde_json::json!({}),
            tokio::time::Instant::now() + Duration::from_secs(5),
        )
        .await
        .ok()?
        .get("token")
        .and_then(serde_json::Value::as_str)
        .map(|token| (socket, token.to_string()))
}

/// Refuse parent handoffs where no authenticated control transport exists.
#[cfg(all(feature = "workflows", not(unix)))]
async fn exchange_parent_grant(_socket: PathBuf, _token: String) -> Option<(PathBuf, String)> {
    None
}

/// Build the capability for one ACP session from that task's own environment.
///
/// The daemon process serves tasks at many depths concurrently, so ambient
/// process variables cannot describe the session being created.
#[cfg(feature = "workflows")]
pub(super) fn session_grant(
    session: &str,
    task_env: &HashMap<String, String>,
    tool_mode: Option<&str>,
    workflows_enabled: bool,
    max_depth: u8,
    max_in_flight: usize,
) -> crate::control_socket::Grant {
    let depth = crate::control_socket::depth_from_env(task_env);
    let families = if workflows_enabled {
        crate::control_socket::ToolFamilies::default()
    } else {
        crate::control_socket::ToolFamilies::fleet_only()
    };
    crate::control_socket::Grant::new(session, depth, max_depth)
        .with_families(families)
        .with_max_in_flight(max_in_flight)
        .with_tool_mode(tool_mode)
}

/// Environment switch selecting ACP instead of legacy provider JSONL.
pub const HARNESS_PROTOCOL_ENV: &str = "MEDULLA_HARNESS_PROTOCOL";

/// Whether this task explicitly requests the ACP transport.
pub(in crate::daemon::providers) fn uses_acp(options: &RunTaskOptions) -> bool {
    options
        .env
        .get(HARNESS_PROTOCOL_ENV)
        .is_some_and(|value| value.eq_ignore_ascii_case("acp"))
}

/// Execute one task through the standard Agent Client Protocol.
pub async fn run_acp_task(options: RunTaskOptions) -> Result<RunTaskResult, String> {
    let agent = agent_for(&options);
    // Read before `options` is picked apart below, and cloned because the
    // session setup runs inside an async move closure.
    #[cfg(feature = "workflows")]
    let tool_mode: Option<String> = options.env.get(crate::mcp::TOOL_MODE_ENV).cloned();
    #[cfg(not(feature = "workflows"))]
    let tool_mode: Option<String> = None;
    // Identifies the grant minted for this session, so it can be revoked when
    // the session ends. A resumed session reuses the agent's own id; a new one
    // has none yet at the moment the servers are offered, so it gets a minted
    // key rather than waiting for an id the request itself is asking for.
    let session_key = options
        .resume_session_id
        .clone()
        .unwrap_or_else(|| format!("acp-{}", uuid::Uuid::new_v4()));
    // Kept outside the closure, which moves its copy, so the grant can be
    // revoked once the session is over however it ended.
    let revoke_key = session_key.clone();
    let task_env = options.env.clone();
    let state = Arc::new(Mutex::new(FoldState::new(options.on_event)));
    let notification_state = state.clone();
    let approve = options.skip_permissions;
    let cwd = PathBuf::from(&options.cwd);
    let resume = options.resume_session_id.clone();
    let prompt = options.prompt.clone();
    let abort = options.abort.clone();
    let provider = options.provider;
    let timeout = Duration::from_millis(options.timeout_ms);

    let result = agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                notification_state.lock().unwrap().fold(notification.update);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _cx| {
                let outcome = if approve {
                    request
                        .options
                        .first()
                        .map(|option| {
                            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                option.option_id.clone(),
                            ))
                        })
                        .unwrap_or(RequestPermissionOutcome::Cancelled)
                } else {
                    RequestPermissionOutcome::Cancelled
                };
                responder.respond(RequestPermissionResponse::new(outcome))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;

            let session_id = match resume {
                Some(id) => {
                    let mut request = LoadSessionRequest::new(id.clone(), cwd.clone());
                    // The same servers a new session gets, and for the same
                    // reason. A loaded session restores the *transcript*, not
                    // the client's tool offer — so omitting this hands the
                    // harness a conversation it remembers and no way to act on
                    // it. For the copilot that is the difference between an
                    // agent that edits the graph and one that can only discuss
                    // it, and it would fail silently: the model would explain
                    // what it would have done.
                    request.mcp_servers =
                        medulla_mcp_servers(tool_mode.as_deref(), &session_key, &task_env).await;
                    connection.send_request(request).block_task().await?;
                    id.into()
                }
                None => {
                    let mut request = NewSessionRequest::new(cwd);
                    // Offer the harness Medulla's own tools. Medulla is the ACP
                    // *client* here, so this is the only way it can hand a
                    // harness anything to call — and it is what lets a coding
                    // agent author a workflow rather than only run one.
                    request.mcp_servers =
                        medulla_mcp_servers(tool_mode.as_deref(), &session_key, &task_env).await;
                    connection
                        .send_request(request)
                        .block_task()
                        .await?
                        .session_id
                }
            };

            let request = connection
                .send_request(PromptRequest::new(
                    session_id.clone(),
                    vec![ContentBlock::from(prompt)],
                ))
                .block_task();
            let idle_state = state.clone();
            let idle = async move {
                loop {
                    let deadline = idle_state.lock().unwrap().last_activity + timeout;
                    tokio::time::sleep_until(deadline.into()).await;
                    if Instant::now().duration_since(idle_state.lock().unwrap().last_activity)
                        >= timeout
                    {
                        break;
                    }
                }
            };
            tokio::pin!(request);
            tokio::pin!(idle);
            tokio::select! {
                result = &mut request => {
                    result?;
                }
                _ = abort.cancelled() => {
                    connection
                        .send_notification(CancelNotification::new(session_id.clone()))
                        ?;
                    return Err(AcpError::request_cancelled());
                }
                _ = &mut idle => {
                    connection.send_notification(CancelNotification::new(session_id.clone()))?;
                    return Err(AcpError::new(
                        -32603,
                        format!("ACP task idle for {}ms (no updates)", timeout.as_millis()),
                    ));
                }
            }

            let state = state.lock().unwrap();
            Ok(RunTaskResult {
                provider,
                reply: state.reply(),
                events: state.events,
                usage: None,
                session_id: Some(session_id.to_string()),
            })
        })
        .await;

    // The session is over, so its grant should stop working — a token that
    // outlived the harness it was minted for is a capability nobody is holding.
    // Tasks it already dispatched keep running: killing a working agent because
    // the turn that asked for it finished would discard real work.
    if let Some(plane) = crate::control_socket::active() {
        plane.grants.revoke(&revoke_key);
    }

    result.map_err(|error| format!("{} ACP error: {error}", provider.as_str()))
}

/// Construct the ACP server command for a supported harness.
fn agent_for(options: &RunTaskOptions) -> AcpAgent {
    let config = match options.provider {
        HarnessProvider::Claude => {
            AcpAgentConfig::new("npx").args(["-y", "@agentclientprotocol/claude-agent-acp@latest"])
        }
        HarnessProvider::Codex => {
            AcpAgentConfig::new("npx").args(["-y", "@agentclientprotocol/codex-acp@latest"])
        }
        HarnessProvider::Opencode => AcpAgentConfig::new(crate::protocol::env::provider_bin(
            HarnessProvider::Opencode,
            &options.env,
        ))
        .arg("acp"),
    };
    AcpAgent::new(config.envs(acp_env(options)))
}

/// The environment handed to the ACP agent process.
///
/// The agent process is the one that ends up running `git commit`, so the
/// attribution hook env has to land here too: direct provider runs apply it at
/// their spawn seam, which ACP dispatch bypasses.
///
/// The same is true of a configured `[router]`. ACP dispatch is chosen by
/// transport (`MEDULLA_HARNESS_PROTOCOL=acp`), not by endpoint, so an operator
/// who pointed this worker at a custom endpoint means it for ACP runs as well —
/// and for OpenRouter that endpoint is the local attribution proxy, which an
/// unrouted ACP agent would walk straight past.
///
/// A configured `apiKeyEnv` whose variable is unset is *not* fatal here, unlike
/// the direct spawn seam: the ACP server may hold its own credentials, and this
/// path has no error frame to surface a refusal through. The endpoint is applied
/// and the key left to the agent.
pub(super) fn acp_env(options: &RunTaskOptions) -> HashMap<String, String> {
    let mut env = options.env.clone();
    // A fleet capability belongs only to the per-session MCP subprocess. The
    // ACP agent itself inherits this map, so retaining an ambient pair here
    // would let it redeem a grant minted for another process or session.
    env.remove(crate::control_socket::MCP_SOCKET_ENV);
    env.remove(crate::control_socket::MCP_GRANT_ENV);
    env.remove(crate::control_socket::MCP_PARENT_SOCKET_ENV);
    env.remove(crate::control_socket::MCP_PARENT_GRANT_ENV);
    let attribution_env = crate::attribution::attribution_env(options.attribution, &env);
    env.extend(attribution_env);
    if let Some(router) = &options.router {
        let injection = crate::protocol::env::router_env(options.provider, router);
        for (key, value) in injection.env {
            env.insert(key, value);
        }
        for (child_var, source_name) in injection.secret_env {
            if let Some(secret) = options.env.get(&source_name).filter(|v| !v.is_empty()) {
                env.insert(child_var, secret.clone());
            }
        }
    }
    env
}
