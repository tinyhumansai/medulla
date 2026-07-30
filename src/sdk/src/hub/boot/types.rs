//! Data types for the `boot` module.
#[allow(unused_imports)]
use super::*;
/// One worker the hub fronts on the backend roster.
#[derive(Debug, Clone)]
pub struct WorkerSpec {
    /// The `agentId` the backend targets (defaults to the tiny.place address).
    pub id: String,
    /// The worker's tiny.place address (base58 cryptoId or `@handle`).
    pub address: String,
    /// Display name for the roster entry.
    pub name: String,
    /// Free-text description / capability summary.
    pub description: String,
    /// The coding-agent harness the worker runs (`claude`/`codex`/`opencode`).
    pub harness: String,
    /// Absolute path of the workspace this worker runs tasks in, when the hub
    /// knows it — which is the case for a host in this same process, and not for
    /// a remote tiny.place peer the operator merely named.
    ///
    /// Advertised as `metadata.workspace`; see [`HubWorker::workspace`].
    pub workspace: Option<String>,
}
/// Everything [`start_hub`] needs to bridge the backend to remote workers.
/// Not `Debug`: the log sink is a boxed closure with no useful representation,
/// and the JWT should not be printable by accident either.
#[derive(Clone)]
pub struct HubConfig {
    /// Backend Socket.IO base URL (e.g. `https://staging-api.tinyhumans.ai`).
    pub backend_url: String,
    /// JWT for the Socket.IO handshake (from `medulla login`).
    pub jwt: String,
    /// tiny.place identity directory (the hub's own wallet).
    pub identity_dir: PathBuf,
    /// The workers to advertise initially (may be empty; add more at runtime).
    pub workers: Vec<WorkerSpec>,
    /// The agent-role catalog, for resolving the roles a worker is toggled on
    /// for into the tags and description it advertises.
    ///
    /// Passed in rather than read here: the catalog is layered config the TUI
    /// already loads, and a second read could disagree with what the operator
    /// is looking at on the Agent Templates page.
    pub agent_templates: Vec<crate::runtime::AgentTemplate>,
    /// How often the runner drains the encrypted inbox.
    pub poll: Duration,
    /// Where diagnostics go. Defaults to stderr; a TUI supplies its own so the
    /// hub never writes over a screen it does not own.
    pub log: super::super::types::HubLog,
    /// Where roster changes are saved. `None` keeps the roster in memory only.
    pub persist: Option<super::super::types::RosterSink>,
    /// The device-local bus this hub also dispatches over.
    ///
    /// `Some` makes the hub bi-modal: a worker whose address is bound on this
    /// network is reached in-process, and everything else still goes over
    /// tiny.place. That is what lets one program be both the orchestrator and a
    /// host for its own machine. `None` keeps every worker remote.
    pub local_network: Option<crate::bridge::LocalBridgeNetwork>,
    /// The address the hub binds on the local bus. Ignored without a
    /// [`local_network`](Self::local_network); empty falls back to
    /// [`DEFAULT_LOCAL_HUB_ADDRESS`](super::DEFAULT_LOCAL_HUB_ADDRESS).
    pub local_address: String,
    /// How untargeted tasks choose among provider subscriptions on a host.
    pub subscription_strategy: crate::runtime::SubscriptionRoutingStrategy,
    /// This host's saved workflows, as the cloud plane's store side.
    ///
    /// `Some` advertises them to the orchestrator on every (re)connect and
    /// serves the reads and the authoring turn it round-trips back —
    /// [`crate::workflows::StoreWorkflowBridge`] is the implementation this host
    /// ships. `None` advertises nothing and refuses (rather than drops) any
    /// workflow request that still arrives.
    ///
    /// Install it only when workflows are *enabled*: the bridge is a view of the
    /// store and applies no policy, so advertising graphs this host would refuse
    /// to run only teaches the orchestrator to delegate work that will bounce.
    pub workflows: Option<crate::hub::WorkflowPlane>,
}
/// A running hub: the live [`HubHandle`] plus the client/runner kept alive for
/// the session (dropping this disconnects and stops the pump).
pub struct HubSession {
    /// Live roster control (add/remove/list workers), re-registering on change.
    pub handle: HubHandle,
    pub(super) _runner: Arc<TaskRunner>,
    pub(super) _client: Client,
    /// The inbound-pairing poll, stopped with the session.
    pub(super) _pairing: super::super::pairing::PairingPoll,
}
