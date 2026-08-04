//! Handing a harness Medulla spawns the tools it is meant to have.
//!
//! Two transports start harnesses, and until now only one of them attached this
//! server:
//!
//! * **ACP** — Medulla is the client, so `session/new`'s `mcpServers` carries
//!   the offer ([`crate::daemon::providers::acp`]).
//! * **PTY** — the harness is started the way a person starts it, as a CLI on a
//!   pseudo-terminal. There is no `session/new` to put an offer in, so the
//!   registration has to go on the child's own argv. Nothing did that, which is
//!   why an operator sitting in a Medulla-spawned Claude Code session had no
//!   `workflow_*` or `fleet_*` tools at all: the skills told the model to call
//!   `workflow_run` and the tool was not there.
//!
//! Everything both transports agree on lives here — whether a server is worth
//! attaching, which grant the session gets, and what the child is told to run —
//! so the two doors cannot drift into serving different tool surfaces.
//!
//! # Where the secret goes
//!
//! The fleet grant is a bearer token, and two different processes must never
//! see it: another local user, and any of the harness's *other* children.
//!
//! It cannot go on argv: `/proc/<pid>/cmdline` is world-readable on Linux while
//! `/proc/<pid>/environ` is readable only by the owner, so an argv registration
//! would hand every local user this session's capability. The registration
//! itself (binary path, `mcp` argument, tool mode) holds no secret and goes on
//! argv freely.
//!
//! Nor can it go on the *harness's own environment*, the way [`Self::env`]
//! does: that environment is inherited by every subprocess the harness spawns,
//! not just the `medulla mcp` server it is meant for — a configured
//! provider-binary override, a shell command, or another project's MCP server
//! sharing that environment could redeem it against the control socket for as
//! long as the session runs. So [`ServerSpec::secret_env`] instead reaches
//! Claude Code through a `--mcp-config` *file* ([`ServerSpec::write_config_file`])
//! that only its owner can read, naming the grant in the one server entry's own
//! `env` block. Claude Code applies a server's `env` only when spawning that
//! server, so the grant reaches the one subprocess it is for and nothing else
//! the harness starts. [`revoke_session`] removes the file along with the
//! grant it names.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::protocol::HarnessProvider;

/// A stdio MCP server, described once and rendered per transport.
///
/// Deliberately not an ACP type: the PTY path has no ACP types in scope, and
/// making the shared description depend on the ACP schema would mean the
/// agreement between the two transports lives in one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSpec {
    /// The server key a harness lists this under, and the prefix a model sees
    /// in every tool name (`mcp__medulla__workflow_run`).
    pub name: &'static str,
    /// The binary to launch — this same Medulla executable.
    pub command: PathBuf,
    /// Its argv: `mcp`.
    pub args: Vec<String>,
    /// Environment for the server process that carries no secret, so it is safe
    /// to render into a registration a transport may put on a command line.
    pub env: Vec<(String, String)>,
    /// Environment that *is* secret — the fleet grant and the socket it is
    /// redeemed at. Kept apart from [`Self::env`] so a transport cannot leak it
    /// onto an argv by treating all of the server's environment alike.
    pub secret_env: Vec<(String, String)>,
}

impl ServerSpec {
    /// The `--mcp-config` document registering this server with Claude Code.
    ///
    /// Only the non-secret environment is rendered; [`Self::secret_env`] reaches
    /// the server by inheritance from the harness process instead. See the
    /// module docs for why.
    ///
    /// Claude Code merges `--mcp-config` with the servers it discovers itself
    /// (`.mcp.json`, user scope) unless `--strict-mcp-config` is also passed,
    /// which is why this adds Medulla's server without displacing the
    /// operator's.
    pub fn claude_mcp_config(&self) -> String {
        let mut entry = Map::new();
        entry.insert(
            "command".to_string(),
            Value::String(self.command.display().to_string()),
        );
        entry.insert(
            "args".to_string(),
            Value::Array(self.args.iter().cloned().map(Value::String).collect()),
        );
        if !self.env.is_empty() {
            let env: Map<String, Value> = self
                .env
                .iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect();
            entry.insert("env".to_string(), Value::Object(env));
        }
        json!({ "mcpServers": { self.name: Value::Object(entry) } }).to_string()
    }

    /// Write this server's full registration — [`Self::secret_env`] included —
    /// to a file only its owner can read, and return the path.
    ///
    /// This is how the secret half reaches Claude Code without ever landing on
    /// an argv or the harness's own environment; see the module docs for why
    /// both of those are unsafe for it. `--mcp-config` accepts a file path as
    /// readily as an inline document, and Claude Code applies a server's `env`
    /// only when spawning *that* server — so a grant recorded here reaches the
    /// `medulla mcp` subprocess and nothing else the harness starts.
    ///
    /// Named after `session` so [`revoke_session`] can find and remove it with
    /// no plumbing back from wherever this was called: reconstructing the same
    /// path from the same key is enough.
    ///
    /// # Errors
    ///
    /// Whatever creating or writing the file returns. The caller's fallback is
    /// the non-secret inline document — see [`attach_cli`].
    pub fn write_config_file(&self, session: &str) -> std::io::Result<PathBuf> {
        let mut entry = Map::new();
        entry.insert(
            "command".to_string(),
            Value::String(self.command.display().to_string()),
        );
        entry.insert(
            "args".to_string(),
            Value::Array(self.args.iter().cloned().map(Value::String).collect()),
        );
        let mut env: Map<String, Value> = self
            .env
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect();
        for (key, value) in &self.secret_env {
            env.insert(key.clone(), Value::String(value.clone()));
        }
        if !env.is_empty() {
            entry.insert("env".to_string(), Value::Object(env));
        }
        let document = json!({ "mcpServers": { self.name: Value::Object(entry) } }).to_string();
        let path = config_file_path(session);
        write_owner_only(&path, &document)?;
        Ok(path)
    }
}

/// Where a session's [`ServerSpec::write_config_file`] output would be, if it
/// wrote one.
///
/// Deterministic from the session key alone: [`revoke_session`] needs to find
/// and remove the same file without any caller having to hand the path back.
fn config_file_path(session: &str) -> PathBuf {
    std::env::temp_dir().join(format!("medulla-mcp-{session}.json"))
}

/// Write `contents` to `path`, created with permissions only its owner can
/// read.
///
/// On Unix the mode is set at creation (`O_CREAT` with the mode already
/// applied), not with a follow-up `chmod`, so there is no window where the
/// file briefly exists world-readable. Windows has no equivalent bit; the
/// threat this defends against — `/proc/<pid>/environ` and world-readable
/// temp files — is a Unix one, as the module docs describe.
#[cfg(unix)]
fn write_owner_only(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents.as_bytes())
}

/// Windows fallback: no owner-only creation mode to apply.
#[cfg(not(unix))]
fn write_owner_only(path: &Path, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

/// Whether workflow authoring is switched on for this host.
///
/// Resolved through the config the *parent* chose (`--config`, recorded in
/// [`crate::config::CONFIG_PATH_ENV`]) rather than rediscovered, so a policy the
/// operator set explicitly is not overridden by whatever a child's working
/// directory happens to find. An unreadable config is treated as enabled, which
/// matches [`crate::config`]'s own default.
pub fn workflows_enabled(env: &HashMap<String, String>) -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    crate::config::load_config(crate::config::explicit_config_from_env(env), env, &cwd)
        .map(|loaded| loaded.config.workflows.enabled)
        .unwrap_or(true)
}

/// Build the capability for one session from that session's own environment.
///
/// A process serves sessions at many depths concurrently, so ambient process
/// variables cannot describe the one being created — the depth is read from the
/// environment the *task* was dispatched with and recorded in the grant, so
/// every later check consults the grant rather than an environment the harness
/// itself could rewrite.
pub fn session_grant(
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

/// Mint this session's fleet grant against the control plane *this* process
/// serves, if it serves one.
///
/// `None` on every process that bound no control plane — a remote worker's
/// daemon, a `medulla workflow` invocation, a host with `mcp.fleetTools` off —
/// and that is exactly what leaves those sessions with no fleet verbs. Minted
/// per session rather than inherited: an inherited token would hand a second
/// harness the first one's capability.
pub fn local_fleet_grant(
    session: &str,
    task_env: &HashMap<String, String>,
    tool_mode: Option<&str>,
    workflows_enabled: bool,
) -> Option<(PathBuf, String)> {
    let plane = crate::control_socket::active()?;
    let grant = session_grant(
        session,
        task_env,
        tool_mode,
        workflows_enabled,
        plane.max_depth,
        plane.max_in_flight,
    );
    Some((plane.socket.clone(), plane.grants.mint(grant)))
}

/// Give back a session's grant once that session is over, and remove the
/// `--mcp-config` file it may have been minted into.
///
/// Best effort by design: a process with no control plane has nothing to
/// revoke against, a session that ended twice must not be an error, and a
/// session that never wrote a config file (no fleet grant, or the inline
/// document was enough) leaves nothing to remove. Called when a harness exits
/// so its token stops working, rather than living until the process does.
pub fn revoke_session(session: &str) {
    if let Some(plane) = crate::control_socket::active() {
        plane.grants.revoke(session);
    }
    let _ = std::fs::remove_file(config_file_path(session));
}

/// The server to offer a session, or `None` when no family would be served.
///
/// `fleet` is the grant the caller resolved — locally minted
/// ([`local_fleet_grant`]) or, for ACP, exchanged from a verified parent
/// handoff. With no fleet grant *and* workflows switched off there is nothing
/// left to serve, so no server is attached rather than one that answers every
/// call with a refusal.
///
/// Returns `None` too when this binary's own path cannot be resolved: a
/// registration pointing at a command we could not name would fail in the
/// harness, where the operator has no way to see why.
pub fn server_spec(
    tool_mode: Option<&str>,
    fleet: Option<(PathBuf, String)>,
    workflows_enabled: bool,
) -> Option<ServerSpec> {
    if !workflows_enabled && fleet.is_none() {
        return None;
    }
    let command = std::env::current_exe().ok()?;
    let mut spec = ServerSpec {
        name: super::SERVER_NAME,
        command,
        args: vec!["mcp".to_string()],
        env: Vec::new(),
        secret_env: Vec::new(),
    };
    if let Some((socket, token)) = fleet {
        spec.secret_env.push((
            crate::control_socket::MCP_SOCKET_ENV.to_string(),
            socket.to_string_lossy().into_owned(),
        ));
        spec.secret_env
            .push((crate::control_socket::MCP_GRANT_ENV.to_string(), token));
    }
    // Passed explicitly rather than left to inheritance, because it is the one
    // setting that differs per session: one process serves authoring turns and
    // review turns, so an inherited value could only ever be right for one of
    // them.
    if let Some(mode) = tool_mode {
        let (mode, scope) = mode
            .split_once(':')
            .map_or((mode, None), |(mode, scope)| (mode, Some(scope)));
        spec.env
            .push((super::TOOL_MODE_ENV.to_string(), mode.to_string()));
        if let Some(scope) = scope {
            spec.env
                .push((super::TOOL_SCOPE_ENV.to_string(), scope.to_string()));
        }
    }
    Some(spec)
}

/// Attach Medulla's tools to a harness about to be started as a **CLI**.
///
/// Mutates the spawn the caller is assembling: secret environment into `env`,
/// the registration onto `args`. Returns the grant's session key so the caller
/// can [`revoke_session`] it when the harness exits, or `None` when nothing was
/// attached.
///
/// `session` is the key the grant is minted under and must be unique per
/// launch — two sessions sharing one key would share one capability, and
/// revoking either would revoke both.
///
/// Nothing is attached for a provider whose CLI has no registration flag we
/// have verified; see [`supports_cli_attach`]. That is a real gap rather than a
/// silent one: those providers still receive the tools over ACP, which is the
/// transport the orchestrator uses for delegated work.
///
/// `env` is read to resolve the session's policy (`workflows.enabled`, the
/// operator's tool mode) but never written: the secret half of a granted
/// session goes into a [`ServerSpec::write_config_file`] file, not this
/// process's or the harness's environment — see the module docs.
pub fn attach_cli(
    provider: HarnessProvider,
    session: &str,
    env: &mut HashMap<String, String>,
    args: &mut Vec<String>,
) -> Option<String> {
    if !supports_cli_attach(provider) {
        return None;
    }
    let workflows_on = workflows_enabled(env);
    let tool_mode = env.get(super::TOOL_MODE_ENV).cloned();
    let fleet = local_fleet_grant(session, env, tool_mode.as_deref(), workflows_on);
    let granted = fleet.is_some();
    let Some(spec) = server_spec(tool_mode.as_deref(), fleet, workflows_on) else {
        // A grant was minted for a session that will not carry it. Given back
        // rather than left in the registry, where it would be a live token
        // nothing can ever redeem.
        if granted {
            revoke_session(session);
        }
        return None;
    };
    args.push("--mcp-config".to_string());
    if spec.secret_env.is_empty() {
        // Nothing secret to keep off argv or out of the harness's own
        // environment, so the simplest registration — inline, no file to
        // clean up later — is also the safe one.
        args.push(spec.claude_mcp_config());
        return granted.then(|| session.to_string());
    }
    match spec.write_config_file(session) {
        Ok(path) => {
            args.push(path.display().to_string());
            granted.then(|| session.to_string())
        }
        Err(_) => {
            // Could not write the file the grant would have gone in. The
            // inline document is still safe to fall back to — it never
            // renders `secret_env` — but a session with no way to reach its
            // grant has no use for one, so it is given back rather than left
            // a live token nothing can ever redeem.
            args.push(spec.claude_mcp_config());
            if granted {
                revoke_session(session);
            }
            None
        }
    }
}

/// Whether a provider's CLI can be handed an MCP registration on its argv.
///
/// Only Claude Code today: `--mcp-config` takes a JSON document and *merges*
/// with the harness's own configured servers, which is exactly the shape this
/// needs. Codex configures its servers through `~/.codex/config.toml`, where the
/// equivalent is a per-launch `-c` override this has not verified against a real
/// CLI — and a registration that silently does not take is worse than an
/// honest absence, so it is not guessed at. `medulla skills --with-mcp` remains
/// the supported way to register Codex.
pub fn supports_cli_attach(provider: HarnessProvider) -> bool {
    matches!(provider, HarnessProvider::Claude)
}

#[cfg(test)]
#[path = "attach_tests.rs"]
mod tests;
