//! Data types for the `boot` module.
#[allow(unused_imports)]
use super::*;

use medulla_link::keys::{NodeId, PairKey};

/// One enrolled peer on the host link.
///
/// Carries all three identifiers because they come from different places and
/// none is derivable from the others: the name is the registry's, the id is the
/// backend's, and the pair key is the user's (protocol §7.1) and never passed
/// through an API.
#[derive(Debug, Clone)]
pub struct HubLinkPeer {
    /// The bridge address callers use.
    pub name: String,
    /// The 16-byte identifier that travels on the wire.
    pub node_id: NodeId,
    /// The end-to-end key shared with this peer.
    pub pair_key: PairKey,
}

/// How the hub brings up its host link.
#[derive(Clone)]
pub struct HubLinkConfig {
    /// The link identity directory, normally `<medulla_home>/link`.
    pub state_dir: PathBuf,
    /// This endpoint's node name — the address peers send to.
    pub node_name: String,
    /// Overrides the forwarder endpoint recorded in `node.json`.
    pub forwarder_endpoint: Option<String>,
    /// The hosts this orchestrator has enrolled.
    pub peers: Vec<HubLinkPeer>,
    /// An already-open link owned by the embedding process.
    ///
    /// Embedded hubs share this handle with status observation because the
    /// identity directory is exclusively locked. Standalone hubs leave it
    /// unset and open the link during startup.
    pub handle: Option<Arc<medulla_link::LinkHandle>>,
}

/// One worker the hub fronts on the backend roster.
///
/// A projection of an [`AgentDeclaration`](crate::runtime::AgentDeclaration) for
/// a host in this process, and of a remembered roster row for a remote peer.
/// Several specs may share one `address`: a machine advertises one entry per
/// declared agent, and `id` is what tells them apart.
#[derive(Debug, Clone, Default)]
pub struct WorkerSpec {
    /// The `agentId` the backend targets (defaults to the worker's node name).
    pub id: String,
    /// The host this worker runs on. Empty when the hub has no opinion — a
    /// remote peer the operator added by address.
    pub host_id: String,
    /// The worker's bridge address — its link node name, or a device-local name.
    pub address: String,
    /// Display name for the roster entry.
    pub name: String,
    /// Free-text description / capability summary.
    pub description: String,
    /// The coding-agent harness the worker runs (`claude`/`codex`/`opencode`).
    pub harness: String,
    /// The workspace this worker runs tasks in, when the hub knows it — which is
    /// the case for a host in this same process, and not for a remote host the
    /// operator merely named.
    ///
    /// Its path is advertised as `metadata.workspace`; see
    /// [`HubWorker::workspace`](crate::hub::HubWorker::workspace).
    pub workspace: Option<crate::runtime::WorkspaceRef>,
    /// Agent-template ids this worker is offered for. Empty means unspecified.
    ///
    /// Sourced from the agent's declaration, which is what finally gives
    /// `metadata.roles` something to carry for a host in this process.
    pub roles: Vec<String>,
    /// Sessions this worker may run at once, derived from its declared
    /// strategy. Zero means unstated and is normalised to the serial default
    /// when the roster entry is built.
    pub max_sessions: u32,
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
    /// The host link to bring up, or `None` for a device-local-only hub.
    pub link: Option<HubLinkConfig>,
    /// The workers to advertise initially (may be empty; add more at runtime).
    pub workers: Vec<WorkerSpec>,
    /// The agent-role catalog, for resolving the roles a worker is toggled on
    /// for into the tags and description it advertises.
    ///
    /// Passed in rather than read here: the catalog is layered config the TUI
    /// already loads, and a second read could disagree with what the operator
    /// is looking at on the Agent Templates page.
    pub agent_templates: Vec<crate::runtime::AgentTemplate>,
    /// The hosts *this machine* declares, advertised as the payload's `hosts[]`
    /// block and used to decide which advertised agents run locally.
    ///
    /// Shared rather than owned so a host started mid-session joins the same
    /// list the hub reads at registration time (see
    /// [`SharedLocalHosts`](crate::hub::SharedLocalHosts)). Default-empty is the
    /// honest answer for a hub that hosts nothing itself: every agent it fronts
    /// then belongs to another machine, and the block says so.
    pub local_hosts: crate::hub::SharedLocalHosts,
    /// How often the runner drains the inbox.
    pub poll: Duration,
    /// Where diagnostics go. Defaults to stderr; a TUI supplies its own so the
    /// hub never writes over a screen it does not own.
    pub log: super::super::types::HubLog,
    /// Where roster changes are saved. `None` keeps the roster in memory only.
    pub persist: Option<super::super::types::RosterSink>,
    /// The device-local bus this hub also dispatches over.
    ///
    /// `Some` makes the hub bi-modal: a worker whose address is bound on this
    /// network is reached in-process, and everything else still goes over the
    /// host link. That is what lets one program be both the orchestrator and a
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
}
