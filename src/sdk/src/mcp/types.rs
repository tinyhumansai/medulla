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
    /// Where an unsolicited notification goes to reach the client.
    ///
    /// `None` for a session with no transport behind it — every in-process
    /// test, and any embedding that answers requests without a stdout to write
    /// notifications to. Progress is then simply not reported, which is the
    /// pre-notification behaviour and hurts nothing.
    pub(crate) notifications: Option<tokio::sync::mpsc::Sender<serde_json::Value>>,
    /// Serializes the short workflow tools that mutate shared definitions.
    ///
    /// Request handling is concurrent so long fleet polls cannot block aborts.
    /// Definition edits remain read-modify-write operations, however, and must
    /// not read the same base then overwrite one another within this session.
    pub(crate) workflow_mutations: tokio::sync::Mutex<()>,
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
            notifications: None,
            workflow_mutations: tokio::sync::Mutex::new(()),
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

    /// Send this session's unsolicited notifications down `outbound`.
    ///
    /// The same channel the responses go down, because both end up as one line
    /// each on the same stdout and the writer is what serializes them.
    pub(super) fn with_notifications(
        mut self,
        outbound: tokio::sync::mpsc::Sender<serde_json::Value>,
    ) -> Self {
        self.notifications = Some(outbound);
        self
    }

    /// Apply the host workflow switch before a fleet handshake is attempted.
    ///
    /// A successful handshake replaces both family flags with the grant's
    /// authority. If it fails, this keeps a fleet-only host from accidentally
    /// falling back to the local workflow surface.
    pub(super) fn with_workflows_enabled(mut self, enabled: bool) -> Self {
        self.families.workflows = enabled;
        self
    }
}

/// A JSON-RPC error.
pub(crate) struct RpcError {
    /// The JSON-RPC error code.
    pub code: i64,
    /// The human-readable message; for a tool this is what the model reads.
    pub message: String,
}

impl RpcError {
    /// The method is not one this server implements.
    pub(super) fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
        }
    }

    /// The request was understood but its arguments were not usable.
    pub(crate) fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }
}
