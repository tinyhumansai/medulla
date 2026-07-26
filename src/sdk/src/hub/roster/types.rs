//! Data types for the `roster` module.
#[allow(unused_imports)]
use super::*;
/// One worker in the live roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubWorker {
    /// The `agentId` the backend targets (defaults to the address).
    pub id: String,
    /// tiny.place address (base58 cryptoId or `@handle`).
    pub address: String,
    /// Coding-agent harness the worker runs.
    pub harness: String,
    /// Optional human label.
    pub label: Option<String>,
    /// Whether this worker is the currently-selected default.
    pub selected: bool,
    /// Absolute path of the workspace this worker runs tasks in, when known.
    ///
    /// Advertised to the backend as `metadata.workspace`, which is what turns a
    /// bare agent into a placed one: the backend derives a `WorkspaceDescriptor`
    /// from it and hangs the agent off that workspace. Without it the
    /// orchestrator sees an agent with no workspace and no placement, and
    /// reports "no workspaces are declared on this host" — which reads as an
    /// unusable fleet and makes it decline work it could in fact delegate.
    ///
    /// `None` for a remote peer whose working directory this hub has no way to
    /// know; the backend then falls back to the worker's probed `capabilities.cwd`.
    pub workspace: Option<String>,
}
/// The roster shared between the socket layer and the [`HubHandle`].
pub type SharedRoster = Arc<Mutex<Vec<HubWorker>>>;

/// Live subscription-selection policy shared by the socket task path and the
/// operator-facing hub handle.
pub type SharedSubscriptionStrategy = Arc<Mutex<crate::runtime::SubscriptionRoutingStrategy>>;
