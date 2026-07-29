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
    /// Agent-template ids this worker is offered for.
    ///
    /// A worker is a *harness* — a claude or codex daemon. A role is a
    /// *template*: a description, a tool allowlist, a model tier. Naming roles
    /// here says "this machine is available for these", which is what lets the
    /// orchestrator route a matching subtask to it rather than treating every
    /// worker as an interchangeable code runner.
    ///
    /// Empty means unspecified, and is advertised exactly as before — a general
    /// worker, not one excluded from everything.
    pub roles: Vec<String>,
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
