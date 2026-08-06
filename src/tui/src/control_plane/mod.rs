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

#[cfg(all(test, unix))]
use medulla::control_socket::CONTROL_SOCKET_ENV;
#[cfg(unix)]
use medulla::control_socket::{
    control_socket_path, ControlServer, FleetDefaults, FleetOps, HubFleetOps,
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
/// Binds even when `hub` is not yet filled — a login-less or hub-less launch
/// still has locally launched harnesses whose lifecycle hooks need somewhere
/// to report, and `HubFleetOps` already reads the slot per request rather
/// than at construction, so a relogin that fills it later needs no rebind.
/// Only the fleet-facing ops go unanswered until then; `hook.report` carries
/// no authority over the hub at all (see the module docs on
/// `control_socket::server::handle::hooks`).
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
    hook_log: medulla::harness_hooks::HookEventLog,
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
    // The one op on this socket that is not about the fleet: a launched
    // harness's own hooks reporting what just happened to it, into the log the
    // app renders.
    let ops: Arc<dyn FleetOps> = Arc::new(HubFleetOps::new(hub, defaults).with_hook_log(hook_log));
    // Binding preserves every existing parent directory's mode, then rejects
    // any replaceable ancestor. Only a directory it creates is chmodded.
    match ControlServer::bind(&path, ops, Default::default()).await {
        Ok(server) => {
            // A grant this fresh registry never minted can never be redeemed,
            // so any `--mcp-config` file a *previous* run of this account left
            // behind — most often one whose process was killed before it could
            // reap its own sessions — is pure leftover by now.
            //
            // Strictly *before* the install below, and that ordering is the
            // whole correctness argument. `install` publishes
            // `control_socket::active()`, and by this point the hub is already
            // up: another Tokio worker can answer an arriving task and launch
            // a PTY session the instant a plane exists. Sweeping after the
            // publish would race that launch and delete the config file it had
            // just written, leaving a live session pointed at a file that no
            // longer exists — the fleet tools silently absent from a harness
            // that was granted them. Sweeping first means every file this run
            // writes is written after the last deletion this run performs.
            // The socket is passed rather than read back from the plane for
            // the same reason: it is not published yet, and the directory
            // these files live in is keyed off it. `mcp` itself only exists
            // with the `workflows` feature compiled in.
            #[cfg(feature = "workflows")]
            medulla::mcp::sweep_stale_config_files(server.path());
            // Recorded process-wide so the ACP spawn path can mint a grant
            // per session without the registry being threaded through the
            // hub, the daemon, and the provider layer to reach it.
            medulla::control_socket::install(medulla::control_socket::ActiveControlPlane {
                socket: server.path().to_path_buf(),
                grants: server.grants().clone(),
                runs: server.runs().clone(),
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
    hook_log: medulla::harness_hooks::HookEventLog,
    logs: &medulla_tui::log::LogBuffer,
) -> Option<()> {
    let _ = (env, config, hub, local_default_worker, hook_log, logs);
    None
}
