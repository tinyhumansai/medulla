//! ACP client setup and one-shot harness task execution.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, Error as AcpError, InitializeRequest, LoadSessionRequest,
    NewSessionRequest, PermissionOptionKind, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionNotification,
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
        use crate::mcp::attach;
        use agent_client_protocol::schema::v1::{EnvVariable, McpServer, McpServerStdio};

        // A launch serving a workflow `agent` node gets no Medulla tools on
        // either transport — see [`crate::harness_tools`]. Ahead of the grant
        // below so nothing is minted for a session that will not carry it.
        if crate::harness_tools::withheld(task_env) {
            return Vec::new();
        }
        // An operator who turned workflows off should not have harnesses handed
        // tools that would be refused. Resolved through the shared policy so
        // this transport and the CLI one cannot come to disagree about which
        // sessions get a server at all.
        let workflows_enabled = attach::workflows_enabled(task_env);
        // The fleet grant, when this process has a control plane to grant
        // against. Minted per session — an inherited one would hand a second
        // harness the first one's capability. A process with no local plane can
        // still exchange a verified parent handoff for a child grant, which is
        // the one thing this transport does that the CLI one cannot.
        let fleet = match attach::local_fleet_grant(session, task_env, tool_mode, workflows_enabled)
        {
            Some(local) => Some(local),
            None => match crate::control_socket::parent_grant_from_env(task_env) {
                Some((socket, token)) => exchange_parent_grant(socket, token).await,
                None => None,
            },
        };
        let Some(spec) = attach::server_spec(tool_mode, fleet, workflows_enabled)
            .map(|spec| spec.for_session(session))
        else {
            return Vec::new();
        };
        // The subprocess inherits this process's environment, which is what
        // carries MEDULLA_HOME — so the harness edits the same workflow store
        // the operator sees. Only what differs per session is pushed here.
        let mut server = McpServerStdio::new(spec.name, spec.command).args(spec.args.clone());
        for (key, value) in spec.env.iter().chain(spec.secret_env.iter()) {
            server.env.push(EnvVariable::new(key.as_str(), value));
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

/// Environment switch selecting ACP instead of legacy provider JSONL.
pub const HARNESS_PROTOCOL_ENV: &str = "MEDULLA_HARNESS_PROTOCOL";

/// The directory a locally-run lifecycle hook should see for this tool call:
/// the checkout the session is working in *now*, or the launch directory when
/// it never announced a move.
///
/// `tracked` is the session's folded workspace directory (see
/// [`super::fold`]), which is how Medulla learns that a session created a
/// worktree and continued there. A `PostToolUse` auto-commit hook resolves its
/// repository from this, so handing it the launch directory after such a move
/// checkpoints the wrong checkout — the ACP counterpart of what
/// [`crate::core_host::turn_cwd`] fixes for the embedded core.
///
/// A tracked directory that does not exist is ignored rather than passed on:
/// the report may name a worktree that has since been removed, and spawning
/// into a missing directory fails the hook instead of merely misplacing it.
pub(super) fn hook_cwd_for(tracked: Option<&str>, launch: &std::path::Path) -> PathBuf {
    tracked
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| launch.to_path_buf())
}

/// Whether this task explicitly requests the ACP transport.
pub(in crate::daemon::providers) fn uses_acp(options: &RunTaskOptions) -> bool {
    options
        .env
        .get(HARNESS_PROTOCOL_ENV)
        .is_some_and(|value| value.eq_ignore_ascii_case("acp"))
}

/// Execute one task through the standard Agent Client Protocol.
pub async fn run_acp_task(options: RunTaskOptions) -> Result<RunTaskResult, String> {
    // The operator's hooks, in the form this transport can carry them: for
    // Claude Code an `_meta` passenger on the session request, and for the rest
    // a note saying they are not running here. Built before anything is spawned
    // so the note reaches the log even if the session never opens.
    let delivery = crate::harness_hooks::acp_delivery(options.provider, &options.hooks);
    for note in &delivery.notes {
        tracing::warn!(provider = options.provider.as_str(), "{note}");
    }
    let session_meta = delivery.session_meta;
    let local_post_tool_use = delivery.local_post_tool_use;
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
    // ACP starts a server which then starts the harness. Seed its environment
    // before that server launches, so built-in hook commands inherit the
    // per-session reporting capability just as direct spawn paths do. Keep the
    // guard alive for the complete ACP run so a late hook cannot lose its
    // authority while the harness is still running.
    let mut agent_env = acp_env(&options)?;
    let _hook_grant = crate::harness_hooks::seed_hook_grant(&session_key, &mut agent_env);
    // The ACP server process receives `agent_env`; the local PostToolUse
    // fallback spawns hooks from this same process, so it gets the same
    // per-session environment — the hook grant and task-specific router /
    // attribution variables among it — or a hook that reports back finds
    // nothing to report to.
    let hook_env = agent_env.clone();
    let agent = agent_for_with_env(&options, agent_env)?;
    let task_env = options.env.clone();
    let state = Arc::new(Mutex::new(FoldState::with_workspace(
        options.on_event,
        options.workspace_context,
        options.env.contains_key("GH_REPO"),
        options.on_workspace_context,
    )));
    let notification_state = state.clone();
    let hook_cwd = PathBuf::from(&options.cwd);
    // The ACP session id is not known until `session/new` (or `session/load`)
    // answers. Hooks that correlate by session id should see the real id, not
    // the synthetic grant key minted before the request; the shared cell is
    // filled in as soon as the id is learned.
    let hook_session = Arc::new(Mutex::new(session_key.clone()));
    let connection_hook_session = hook_session.clone();
    let approve = options.skip_permissions;
    let cwd = PathBuf::from(&options.cwd);
    let resume = options.resume_session_id.clone();
    let prompt = options.prompt.clone();
    let abort = options.abort.clone();
    let on_session = options.on_session;
    let provider = options.provider;
    let timeout = Duration::from_millis(options.timeout_ms);

    let result = agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                // The session's *current* directory, read under the same lock
                // that just folded the update, so a tool call that moved the
                // session into a worktree is already reflected when this call's
                // own hooks run. `hook_cwd` — the launch directory — is the
                // fallback for a session that never announced a move.
                let (completed, session_cwd) = {
                    let mut state = notification_state.lock().unwrap();
                    let completed = state.fold(notification.update);
                    let cwd = completed
                        .is_some()
                        .then(|| state.workspace_cwd().map(str::to_owned))
                        .flatten();
                    (completed, cwd)
                };
                if let Some(completed) = completed {
                    let session_id = hook_session.lock().unwrap().clone();
                    let cwd = hook_cwd_for(session_cwd.as_deref(), &hook_cwd);
                    crate::harness_hooks::acp::run_post_tool_use(
                        &local_post_tool_use,
                        &cwd,
                        &hook_env,
                        &session_id,
                        &completed.tool_name,
                        &completed.input,
                    )
                    .await;
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _cx| {
                // Selection is by KIND, never by position: option order is the
                // agent's presentation choice, not protocol contract, and a
                // harness that lists a reject option first turns positional
                // "auto-approve" into auto-DENY — observed in the field as a
                // correct `gh` call dying instantly at the permission prompt.
                let outcome = if approve {
                    request
                        .options
                        .iter()
                        .find(|option| matches!(option.kind, PermissionOptionKind::AllowOnce))
                        .or_else(|| {
                            request.options.iter().find(|option| {
                                matches!(option.kind, PermissionOptionKind::AllowAlways)
                            })
                        })
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
                    // A loaded session gets the hooks a new one gets. The ACP
                    // server rebuilds the underlying harness query from these
                    // params, so omitting them here would mean a resumed run —
                    // every retry, every continued workflow step — silently
                    // stopped checkpointing where the first turn did not.
                    request.meta = session_meta;
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
                    // Medulla's hooks, for the harnesses whose ACP server takes
                    // its configuration this way (see
                    // [`crate::harness_hooks::acp`]). `None` for the rest, which
                    // leaves the request exactly as it was.
                    request.meta = session_meta;
                    connection
                        .send_request(request)
                        .block_task()
                        .await?
                        .session_id
                }
            };

            // Publish the real ACP id to hooks before prompting, then announce
            // it to the daemon. Notifications produced by the prompt may
            // immediately report workspace state; the daemon must already have
            // a binding for that callback to update rather than dropping the
            // first turn's context.
            *connection_hook_session.lock().unwrap() = session_id.to_string();
            if let Some(callback) = on_session {
                callback(session_id.to_string());
            }

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
#[cfg(test)]
pub(super) fn agent_for(options: &RunTaskOptions) -> Result<AcpAgent, String> {
    // Built once and shared: the model flags below are derived from the same
    // map the child receives, and `router_env` writing `OPENAI_BASE_URL` into it
    // is what tells `codex_overrides` the run is routed at all.
    let env = acp_env(options)?;
    agent_for_with_env(options, env)
}

/// Construct an ACP server command using a fully prepared child environment.
///
/// The execution path supplies a per-session hook grant in addition to the
/// ordinary ACP environment. Keeping this helper separate preserves
/// [`agent_for`] as the pure configuration entry point used by focused tests.
fn agent_for_with_env(
    options: &RunTaskOptions,
    mut env: HashMap<String, String>,
) -> Result<AcpAgent, String> {
    // Codex's routed provider block reaches its ACP server through the
    // environment, not argv — see `codex_acp_overrides_env`. Applied before the
    // config is built so the overlay below carries it.
    for (key, value) in codex_acp_overrides_env(options, &env)? {
        env.insert(key, value);
    }
    let config = match options.provider {
        HarnessProvider::Claude => {
            AcpAgentConfig::new("npx").args(["-y", "@agentclientprotocol/claude-agent-acp@latest"])
        }
        HarnessProvider::Codex => AcpAgentConfig::new("npx").args(codex_acp_args(options, &env)?),
        // The provider-binary override is untrusted configuration that lands in
        // an `env` argv (Unix) or `cmd.exe` line (Windows). A value containing
        // `=` would be consumed as a variable assignment by `env` even after
        // its `--` terminator — the name could never be executed, and the next
        // argument would silently become the command. Reject it at the boundary
        // instead of hoping the shell misparse is harmless.
        HarnessProvider::Opencode => {
            let bin = crate::protocol::env::provider_bin(HarnessProvider::Opencode, &options.env);
            if bin.contains('=') {
                return Err(
                    "provider binary override must not contain `=`: `env` would treat it as a variable assignment"
                        .to_string(),
                );
            }
            AcpAgentConfig::new(bin).arg("acp")
        }
        HarnessProvider::Openhuman => {
            unreachable!("OpenHuman's operator TUI is not an ACP coding provider")
        }
    };
    // `AcpAgentConfig::envs` overlays an inheriting command instead of clearing
    // it. Run the actual ACP command through `env -u` as well as scrubbing the
    // overlay so the embedded core workspace cannot leak from Medulla's own
    // process environment into an external harness.
    #[cfg(unix)]
    let config = config
        .command_with_env_removals(crate::protocol::env::CORE_STATE_VARS)
        .envs(env);
    #[cfg(windows)]
    let config = config
        .command_with_env_removals(crate::protocol::env::CORE_STATE_VARS)
        .envs(env);

    Ok(AcpAgent::new(config))
}

/// The `codex-acp` argv, including the model and the routed-provider overrides.
///
/// The direct spawn seam builds these in two places — `-m <slug>` in
/// [`crate::daemon::providers::detect::build_resumed_run_args`] and the `-c`
/// block in [`crate::codex_overrides::launch_args`] — and ACP dispatch used to
/// build neither. A preset therefore reached Codex as a bare `codex-acp` with
/// its `model` silently dropped, so Codex ran the operator's own configured
/// default on the operator's own ChatGPT account: the routed endpoint was in the
/// environment but nothing selected it, and the preset's model never appeared in
/// a single upstream request.
fn codex_acp_args(
    options: &RunTaskOptions,
    env: &HashMap<String, String>,
) -> Result<Vec<String>, String> {
    let mut args = vec![
        "-y".to_string(),
        "@agentclientprotocol/codex-acp@latest".to_string(),
    ];
    if let Some(model) = codex_acp_model(options) {
        args.push("-m".to_string());
        args.push(model.to_string());
    }
    // The routed provider block does NOT ride here. `codex-acp` parses argv only
    // for its `login` and `cli` subcommands; in server mode it ignores it and
    // reads `CODEX_CONFIG`/`MODEL_PROVIDER` from the environment instead, which
    // is what `codex_acp_overrides_env` puts there. `-m` is kept because it is
    // harmless either way and documents the intent at the spawn.
    let _ = env;
    Ok(args)
}

/// The routed model for a Codex ACP spawn, trimmed and non-empty.
fn codex_acp_model(options: &RunTaskOptions) -> Option<&str> {
    options
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
}

/// The routed-provider configuration a Codex ACP spawn needs, in the form its
/// server actually reads.
///
/// A routed run cannot safely fall back to the operator's default account or
/// endpoint — its catalog governs the provider's supported tool shapes — so a
/// catalog that cannot be derived is an error before ACP starts, matching the
/// direct spawn seam.
fn codex_acp_overrides_env(
    options: &RunTaskOptions,
    env: &HashMap<String, String>,
) -> Result<Vec<(String, String)>, String> {
    crate::codex_overrides::acp_env(options.provider, codex_acp_model(options), env)
        .map_err(|error| error.to_string())
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
/// A configured `apiKeyEnv` whose variable is unset is a hard error, matching
/// direct execution. Continuing would start Codex with routed-provider
/// overrides but no routed credential, allowing an unrelated inherited key to
/// be sent to the configured gateway.
pub(super) fn acp_env(options: &RunTaskOptions) -> Result<HashMap<String, String>, String> {
    let mut env = options.env.clone();
    crate::protocol::env::scrub_core_state(&mut env, options.provider);
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
            let secret = options
                .env
                .get(&source_name)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    format!(
                        "router API key env var `{source_name}` is not set; \
                         export it or remove apiKeyEnv from [router]"
                    )
                })?;
            env.insert(child_var, secret.clone());
        }
    }
    Ok(env)
}

/// Adds a Unix `env -u` shim around an ACP command.
///
/// The ACP SDK deliberately preserves the ambient process environment, while
/// `AcpAgentConfig` can only add or replace values.  The shim is therefore the
/// launch-level counterpart to [`acp_env`]'s map scrubbing.
#[cfg(unix)]
trait AcpAgentConfigExt {
    /// Return a command whose inherited values in `names` are removed before
    /// it starts the original ACP executable.
    fn command_with_env_removals(self, names: &[&str]) -> Self;
}

#[cfg(unix)]
impl AcpAgentConfigExt for AcpAgentConfig {
    fn command_with_env_removals(self, names: &[&str]) -> Self {
        let mut args = names
            .iter()
            .flat_map(|name| ["-u".to_string(), (*name).to_string()])
            .collect::<Vec<_>>();
        // The provider-binary override is untrusted configuration: without the
        // `--` terminator, a command starting with `-` or containing `=` is
        // eaten by `env` as an option or a variable assignment instead of being
        // executed.
        args.push("--".to_string());
        args.push(self.command().to_string_lossy().into_owned());
        args.extend(self.arguments().iter().cloned());
        AcpAgentConfig::new("env").args(args)
    }
}

/// Adds a Windows `cmd` shim that clears inherited variables before launching
/// an ACP command. `AcpAgentConfig` itself can only overlay values.
#[cfg(windows)]
trait AcpAgentConfigExt {
    /// Return a command which clears `names` from its child environment before
    /// it invokes the original ACP executable.
    fn command_with_env_removals(self, names: &[&str]) -> Self;
}

#[cfg(windows)]
impl AcpAgentConfigExt for AcpAgentConfig {
    fn command_with_env_removals(self, names: &[&str]) -> Self {
        let clears = names
            .iter()
            .map(|name| format!("set {name}="))
            .collect::<Vec<_>>()
            .join(" && ");
        let command = std::iter::once(quote_windows_cmd_arg(&self.command().to_string_lossy()))
            .chain(
                self.arguments()
                    .iter()
                    .map(|argument| quote_windows_cmd_arg(argument)),
            )
            .collect::<Vec<_>>()
            .join(" ");
        AcpAgentConfig::new("cmd.exe").args(["/D", "/S", "/C", &format!("{clears} && {command}")])
    }
}

/// Quote a command argument for `cmd.exe` without allowing its metacharacters
/// to turn an operator-configured provider path into another command.
#[cfg(windows)]
pub(super) fn quote_windows_cmd_arg(argument: &str) -> String {
    let escaped = argument.replace('^', "^^").replace('%', "%%");
    let escaped = escaped
        .chars()
        .flat_map(|character| match character {
            '&' | '|' | '<' | '>' | '(' | ')' => vec!['^', character],
            _ => vec![character],
        })
        .collect::<String>();
    // `cmd.exe` passes doubled quotes through a double-quoted argument as a
    // literal quote. A caret would instead be preserved by `npx`'s Windows
    // wrapper, turning TOML values such as `model_provider="medulla"` into
    // invalid `^"`-prefixed values when Codex parses its `-c` overrides.
    format!("\"{}\"", escaped.replace('"', "\"\""))
}
