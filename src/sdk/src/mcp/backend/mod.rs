//! Where a fleet tool's work actually happens.
//!
//! The MCP server runs as a subprocess the ACP agent spawned, so it has no hub
//! of its own. A fleet tool call therefore either reaches a running Medulla over
//! the control socket ([`proxy`]) or has nothing to reach at all ([`offline`]).
//!
//! Both cases are ordinary. A harness spawned on a remote worker, or on a host
//! whose operator turned fleet tools off, is handed no grant and gets the
//! offline backend — and then the `fleet_*` tools are simply never advertised,
//! so nothing has to fail at call time to communicate it.

mod offline;
mod proxy;

pub use offline::OfflineFleet;
pub use proxy::ProxyFleet;

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::control_socket::{ControlError, Hello};

/// The live fleet, as the tool layer reaches it.
#[async_trait::async_trait]
pub trait FleetBackend: Send + Sync {
    /// What the control plane said about this session, if one was reachable.
    ///
    /// Read once at startup to decide which `fleet_*` tools to advertise:
    /// withholding a verb is the guard, and refusing the call is only the
    /// backstop. `None` means no fleet tools at all.
    fn hello(&self) -> Option<&Hello>;

    /// Issue one control-plane op.
    ///
    /// # Errors
    ///
    /// [`ControlError::NoInstance`] when there is no reachable Medulla, and
    /// whatever the control plane refused the request with otherwise.
    async fn call(&self, op: &str, params: Value) -> Result<Value, ControlError>;
}

/// Build the backend for a spawned server from its environment.
///
/// Connects eagerly, because the answer decides which tools to advertise and
/// `tools/list` may be the very first thing asked. A failure to connect is not
/// an error: it produces the offline backend, and the session runs with the
/// workflow tools alone rather than refusing to start.
pub async fn from_env(env: &HashMap<String, String>) -> Arc<dyn FleetBackend> {
    let Some((socket, token)) = crate::control_socket::grant_from_env(env) else {
        return Arc::new(OfflineFleet);
    };
    #[cfg(unix)]
    {
        match ProxyFleet::connect(&socket, &token).await {
            Ok(proxy) => Arc::new(proxy),
            Err(_) => Arc::new(OfflineFleet),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (socket, token);
        Arc::new(OfflineFleet)
    }
}
