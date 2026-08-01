//! Session state shared by MCP protocol handling and tool dispatch.

use std::sync::Arc;

use super::{FleetBackend, OfflineFleet, ToolMode};
use crate::control_socket::ToolFamilies;
use crate::workflows::WorkflowStore;

/// Everything one MCP session serves from.
///
/// A struct rather than a growing parameter list: the handler needs the store,
/// what this host permits, which slice of the authoring surface this turn gets,
/// which families the grant covers, and the fleet — and five positional
/// arguments is where two of the same type get silently transposed.
pub struct McpSession {
    /// The workflow store. Always local, always present.
    pub store: Arc<dyn WorkflowStore>,
    /// What this host permits a workflow to do.
    pub policy: crate::workflows::ops::HostPolicy,
    /// How much of the authoring surface this turn is served.
    pub mode: ToolMode,
    /// Which tool families the grant covers.
    pub families: ToolFamilies,
    /// The live fleet, or a stand-in that says there is none.
    pub fleet: Arc<dyn FleetBackend>,
}

impl McpSession {
    /// A session serving the workflow tools alone, off the local store.
    ///
    /// What a harness with no fleet grant gets, and what every session got
    /// before the fleet tools existed.
    pub fn local(
        store: Arc<dyn WorkflowStore>,
        policy: crate::workflows::ops::HostPolicy,
        mode: ToolMode,
    ) -> Self {
        McpSession {
            store,
            policy,
            mode,
            families: ToolFamilies::workflows_only(),
            fleet: Arc::new(OfflineFleet),
        }
    }

    /// Attach a fleet, taking the families from whatever grant it redeemed.
    ///
    /// The families come from the backend's handshake rather than from a
    /// caller-supplied argument, so a session cannot be given a family the
    /// control plane did not grant it.
    pub fn with_fleet(mut self, fleet: Arc<dyn FleetBackend>) -> Self {
        if let Some(hello) = fleet.hello() {
            self.families = hello.families;
        }
        self.fleet = fleet;
        self
    }
}
