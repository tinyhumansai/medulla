//! Bringing the control plane up at launch, and handing back what the session needs.
//!
//! [`super::start`] answers one question — is there a socket, and did it bind?
//! Launch needs three more answers around it: which local worker a shim with no
//! host named should be dispatched to, where reported runs are collected, and
//! who holds the server for the life of the process. Doing that inline in
//! `app_loop` meant four `cfg` arms of wiring sitting in the middle of the
//! startup sequence, so it lives here instead.
//!
//! The binding happens once, outside the relogin loop; see the note on
//! [`super::start`] for why rebinding on relogin would race this process's own
//! live socket.

use std::collections::HashMap;

use medulla::control_socket::HarnessRunRegistry;

use crate::hub_relay::{HubSlot, LocalDispatch};

/// The started control plane, for the session to read and the process to hold.
pub(crate) struct ControlPlane {
    /// The bound server, kept alive rather than used.
    ///
    /// Dropping it unbinds the socket, so it is held for the whole process even
    /// though nothing reads it after startup. `()` on a build that binds
    /// nothing.
    _server: Held,
    /// Where harnesses report the workflow runs they start.
    ///
    /// Empty and inert when nothing bound a socket, which is what leaves the
    /// rail unchanged on such a build or platform.
    pub(crate) runs: HarnessRunRegistry,
}

/// What [`super::start`] hands back on this build, if it is called at all.
#[cfg(all(feature = "workflows", unix))]
type Held = Option<medulla::control_socket::ControlServer>;
#[cfg(all(feature = "workflows", not(unix)))]
type Held = Option<()>;
#[cfg(not(feature = "workflows"))]
type Held = ();

/// Bind the control socket and collect what the session reads from it.
///
/// The local default worker is the host entry whose address is this config's
/// own primary: a shim that names no host is dispatched there, because the
/// machine it is running on is the one it means.
#[cfg(feature = "workflows")]
pub(crate) async fn start(
    env: &HashMap<String, String>,
    config: &medulla::config::TuiConfig,
    hub: HubSlot,
    local_dispatch: &LocalDispatch,
    hook_log: medulla::harness_hooks::HookEventLog,
    logs: &medulla_tui::log::LogBuffer,
) -> ControlPlane {
    let primary_address = config.host.effective_address();
    let local_default_worker = local_dispatch
        .hosts
        .iter()
        .find(|worker| worker.address == primary_address)
        .map(|worker| worker.address.clone());
    let server = super::start(env, config, hub, local_default_worker, hook_log, logs).await;
    #[cfg(unix)]
    let runs = server
        .as_ref()
        .map(|server| server.runs().clone())
        .unwrap_or_default();
    #[cfg(not(unix))]
    let runs = HarnessRunRegistry::default();
    ControlPlane {
        _server: server,
        runs,
    }
}

/// Keep the same startup seam on a build without the workflows feature.
///
/// Nothing binds and nothing reports, so the rail draws exactly what it drew
/// before runs existed.
#[cfg(not(feature = "workflows"))]
pub(crate) async fn start(
    env: &HashMap<String, String>,
    config: &medulla::config::TuiConfig,
    hub: HubSlot,
    local_dispatch: &LocalDispatch,
    hook_log: medulla::harness_hooks::HookEventLog,
    logs: &medulla_tui::log::LogBuffer,
) -> ControlPlane {
    let _ = (env, config, hub, local_dispatch, hook_log, logs);
    ControlPlane {
        _server: (),
        runs: HarnessRunRegistry::default(),
    }
}
