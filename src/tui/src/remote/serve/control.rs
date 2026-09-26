//! The control socket a remote host binds for the sessions it serves.
//!
//! A coding agent Medulla starts is owed three things beyond a pty: its tools,
//! the skills describing them, and somewhere for its lifecycle hooks to report.
//! All three are minted against a control socket *in the process that launched
//! it* — [`medulla::control_socket::active`] is process-wide, and a harness
//! redeems its grant over a unix socket on its own machine.
//!
//! So the remote daemon binds one. Without it `attach_mcp` finds no active
//! plane, mints nothing, and a remote `claude` starts with no `workflow_run`, no
//! managed skills, and hooks that report into the void — while a local one gets
//! all three. That difference would be invisible until somebody asked why the
//! agent on the build box could not see the workflows.
//!
//! # What is deliberately *not* served here
//!
//! Fleet dispatch. [`HubFleetOps`] reads its hub from a slot, and this daemon's
//! slot is empty and stays empty: there is no orchestrator on a remote host, and
//! nothing over there to dispatch to. The ops answer hook reports fully and
//! decline fleet requests, which is the same hub-less shape a login-less TUI
//! already has — so this is an existing, documented state rather than a new one.

use std::collections::HashMap;
#[cfg(unix)]
use std::sync::Arc;

use medulla::config::TuiConfig;
#[cfg(unix)]
use medulla::control_socket::{
    control_socket_path, ActiveControlPlane, ControlServer, FleetDefaults, FleetOps, HubFleetOps,
};

/// Bind this host's control socket and publish it process-wide.
///
/// Unix only, because the socket is one: `control_socket::server` is itself
/// `#[cfg(unix)]`. A Windows host still serves sessions — it simply serves them
/// without tools, which is the same thing a unix host with `fleet_tools = false`
/// does, and is reported the same way.
///
/// Returns the server, which must be kept alive: dropping it unbinds the socket
/// and every grant minted against it becomes unredeemable.
///
/// `None` when fleet tools are switched off in config, or the socket cannot be
/// bound. Neither is fatal — a session still starts, just without tools — so the
/// caller logs and carries on rather than refusing to serve.
#[cfg(unix)]
pub async fn start(
    env: &HashMap<String, String>,
    config: &TuiConfig,
    client_dir: &std::path::Path,
    log: impl Fn(String),
) -> Option<ControlServer> {
    if !config.mcp.fleet_tools {
        return None;
    }
    // Per client, not per account. Direct daemons are deliberately one per
    // client, and the account-scoped default is already taken by whatever else
    // is running under this user — an ordinary `medulla`, or another client's
    // daemon. `ControlServer::bind` refuses a live listener, so every daemon
    // after the first would return `None` here and launch its coding sessions
    // with no tools, no managed skills and nowhere to report — silently, and
    // only on a host that happens to be in use.
    //
    // An explicit `mcp.socketPath` still wins: an operator who named a path
    // meant it, and a host serving one client is the case where it works.
    let path = match config.mcp.socket_path.as_deref() {
        Some(_) => match control_socket_path(env, config.mcp.socket_path.as_deref()) {
            Ok(path) => path,
            Err(error) => {
                log(format!("control socket: {error}"));
                return None;
            }
        },
        None => client_dir.join("control.sock"),
    };
    // An empty hub slot, permanently. See the module doc: a remote host has no
    // orchestrator, so fleet ops are declined while hook reporting — which
    // carries no authority over a hub at all — works fully.
    let hub = Arc::new(std::sync::Mutex::new(None));
    let ops: Arc<dyn FleetOps> = Arc::new(HubFleetOps::new(
        hub,
        FleetDefaults {
            worker_address: None,
        },
    ));
    match ControlServer::bind(&path, ops, Default::default()).await {
        Ok(server) => {
            // Published before any session starts, because `attach_mcp` reads it
            // at launch: a session opened before this line would get no grant,
            // and nothing later would give it one.
            medulla::control_socket::install(ActiveControlPlane {
                socket: server.path().to_path_buf(),
                grants: server.grants().clone(),
                runs: server.runs().clone(),
                max_depth: config.mcp.max_depth,
                max_in_flight: config
                    .mcp
                    .effective_max_in_flight(config.workflows.max_parallel_agents),
            });
            log(format!(
                "control socket: serving sessions on {}",
                server.path().display()
            ));
            Some(server)
        }
        Err(error) => {
            log(format!("control socket: not bound ({error})"));
            None
        }
    }
}

/// No control socket off unix: there is no unix socket to bind.
///
/// Returns `()` rather than an `Option<ControlServer>` because the type does not
/// exist on this platform. The caller holds the result for its lifetime either
/// way, so a unit is exactly the right shape for "nothing to hold".
#[cfg(not(unix))]
pub async fn start(
    env: &HashMap<String, String>,
    config: &TuiConfig,
    client_dir: &std::path::Path,
    log: impl Fn(String),
) {
    let _ = (env, config, client_dir);
    log("control socket: not bound (unix only), so sessions start without tools".to_string());
}
