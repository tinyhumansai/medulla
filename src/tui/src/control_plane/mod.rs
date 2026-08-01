//! Binding the control socket, so spawned harnesses can reach this fleet.
//!
//! Gate and wiring only. Everything the socket actually does lives in
//! [`medulla::control_socket`]; this module decides whether to bind it at all,
//! resolves the path, and keeps the handle alive for the session.
//!
//! Where it binds matters. The socket belongs to the *process*, not to a login
//! session: it is bound once, outside the relogin loop, because rebinding on
//! relogin would race this process's own live socket. The server reads the hub
//! slot per request, so a relogin that refills that slot is picked up with no
//! rebind at all.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
use medulla::control_socket::{
    control_socket_path, ControlServer, FleetDefaults, FleetOps, HubFleetOps, CONTROL_SOCKET_ENV,
};

use crate::hub_relay::HubSlot;

/// Bind the control socket for this process, when it is wanted and possible.
///
/// `None` — and a working TUI either way — when fleet tools are switched off,
/// when the platform has no unix sockets, when no path resolves, or when
/// another live instance already holds the address. That last case is a real
/// possibility on a machine running two Medullas, and first-binder-wins is the
/// deliberate policy: a shim must reach exactly one fleet, and quietly serving
/// two would make "which fleet did my task go to" unanswerable.
///
/// Diagnostics go to `logs`, never to the terminal — ratatui owns the alternate
/// screen and only repaints the cells it manages, so a stray line lands on top
/// of the UI and is never cleared.
#[cfg(unix)]
pub(crate) async fn start(
    env: &HashMap<String, String>,
    config: &medulla::config::TuiConfig,
    hub: HubSlot,
    local_default_worker: Option<String>,
    logs: &medulla_tui::log::LogBuffer,
) -> Option<ControlServer> {
    if !config.mcp.fleet_tools {
        return None;
    }
    let configured = config.mcp.socket_path.as_deref();
    let path = match control_socket_path(env, configured) {
        Ok(path) => path,
        Err(err) => {
            logs.push(format!("control socket: {err}"));
            return None;
        }
    };
    let defaults = FleetDefaults {
        worker_address: local_default_worker,
    };
    let ops: Arc<dyn FleetOps> = Arc::new(HubFleetOps::new(hub, defaults));
    // An operator who named an explicit path in config or the environment
    // may have their reasons for an unusual directory; the default path is
    // ours, and is checked.
    let trusted_path = configured.is_some()
        || env
            .get(CONTROL_SOCKET_ENV)
            .is_some_and(|value| !value.trim().is_empty());
    match ControlServer::bind(&path, ops, Default::default(), trusted_path).await {
        Ok(server) => {
            // Recorded process-wide so the ACP spawn path can mint a grant
            // per session without the registry being threaded through the
            // hub, the daemon, and the provider layer to reach it.
            medulla::control_socket::install(medulla::control_socket::ActiveControlPlane {
                socket: server.path().to_path_buf(),
                grants: server.grants().clone(),
                max_depth: config.mcp.max_depth,
                max_in_flight: config
                    .mcp
                    .effective_max_in_flight(config.workflows.max_parallel_agents),
            });
            logs.push(format!(
                "control socket: serving spawned harnesses on {}",
                server.path().display()
            ));
            Some(server)
        }
        Err(err) => {
            logs.push(format!("control socket: not bound ({err})"));
            None
        }
    }
}

/// Keep the same startup seam on platforms without Unix-domain control sockets.
#[cfg(not(unix))]
pub(crate) async fn start(
    env: &HashMap<String, String>,
    config: &medulla::config::TuiConfig,
    hub: HubSlot,
    local_default_worker: Option<String>,
    logs: &medulla_tui::log::LogBuffer,
) -> Option<()> {
    let _ = (env, config, hub, local_default_worker, logs);
    None
}
