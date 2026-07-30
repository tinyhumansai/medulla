//! Data types for the `roster` module.
#[allow(unused_imports)]
use super::*;
/// One worker in the live roster.
///
/// `Default` exists so callers can spread (`..Default::default()`) rather than
/// restating every field: this struct grows as the advert learns to carry more,
/// and each addition should not break every construction site in the tree. The
/// default worker is not a usable one — `id` and `address` are empty — so it is
/// a starting point to fill in, never a worker to advertise.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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
    /// Who holds this worker's harness right now.
    ///
    /// Advertised as `metadata.control`, and **only** when a person holds it:
    /// absent means the orchestrator has it. Omitting the common case keeps the
    /// advert byte-stable, which matters because it is re-emitted on every
    /// roster mutation.
    pub control: super::super::HandoffControl,
    /// Why a person holds it, when they said. Rendered as-is beside the hold.
    pub control_reason: Option<String>,
    /// Epoch ms the current hold began.
    pub control_since: Option<i64>,
    /// The brief from the most recent handback, until it is superseded.
    ///
    /// Cleared when the operator takes the harness back: a handoff on a harness
    /// somebody has re-taken is a stale invitation, and advertising it would have
    /// the orchestrator plan work into a workspace it cannot enter.
    pub handoff: Option<super::super::HarnessHandoff>,
}
/// The roster shared between the socket layer and the [`HubHandle`].
pub type SharedRoster = Arc<Mutex<Vec<HubWorker>>>;

/// Live subscription-selection policy shared by the socket task path and the
/// operator-facing hub handle.
pub type SharedSubscriptionStrategy = Arc<Mutex<crate::runtime::SubscriptionRoutingStrategy>>;
