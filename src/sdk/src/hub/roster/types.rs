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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubWorker {
    /// The `agentId` the backend targets (defaults to the address).
    pub id: String,
    /// The host this agent runs on.
    ///
    /// One machine now advertises one entry per **declared agent** rather than
    /// one per machine, so the thing that says which machine an entry is on can
    /// no longer be the entry's own address — several agents share it. Empty
    /// means "this hub did not say", which is what a remote peer added by
    /// address looks like; the backend then synthesizes a host for it exactly as
    /// it does today.
    ///
    /// Advertised as the agent's `hostId`, and as one entry in the payload's
    /// `hosts[]` block. Blank is omitted from both, which is what leaves the
    /// backend's `host:${socketId}` synthesis in place for a peer nobody placed.
    pub host_id: String,
    /// link address (node name or `@handle`).
    ///
    /// Not an identity: N declared agents on one machine share one address, and
    /// [`id`](Self::id) is what distinguishes them.
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
    /// The workspace this worker runs tasks in, when known: its path and its
    /// declared type.
    ///
    /// The path is advertised to the backend as `metadata.workspace`, which is
    /// what turns a bare agent into a placed one: the backend derives a
    /// `WorkspaceDescriptor` from it and hangs the agent off that workspace.
    /// Without it the orchestrator sees an agent with no workspace and no
    /// placement, and reports "no workspaces are declared on this host" — which
    /// reads as an unusable fleet and makes it decline work it could in fact
    /// delegate. The *type* rides along in memory only for now; sending
    /// `workspace` as an object is deferred with the remote-host work.
    ///
    /// `None` for a remote peer whose working directory this hub has no way to
    /// know; the backend then falls back to the worker's probed `capabilities.cwd`.
    ///
    /// Was a bare `Option<String>` before agents were declared. Readers that
    /// only want the path use [`workspace_path`](Self::workspace_path), which is
    /// the same answer the old field gave.
    pub workspace: Option<crate::runtime::WorkspaceRef>,
    /// Sessions this agent may run at once, derived from its declared strategy
    /// (`checkout` ⇒ 1, serial).
    ///
    /// Never configured directly: a number an operator could raise past what the
    /// workspace can take is a number that corrupts a checkout. Code-plane data
    /// — deterministic placement reads it, and no prompt ever does (spec §4.2).
    ///
    /// Advertised as `metadata.maxSessions`; a zero is withheld, since a
    /// capacity of nothing reads as saturated.
    pub max_sessions: u32,
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

impl Default for HubWorker {
    /// Written out rather than derived so `max_sessions` starts at the serial
    /// `checkout` bound instead of `0`. A worker spread from the default and
    /// never told its strategy would otherwise advertise a capacity of nothing,
    /// which placement reads as "saturated" — the opposite of the permissive
    /// default every other field here takes.
    fn default() -> Self {
        Self {
            id: String::new(),
            host_id: String::new(),
            address: String::new(),
            harness: String::new(),
            label: None,
            selected: false,
            roles: Vec::new(),
            workspace: None,
            max_sessions: crate::runtime::WorkspaceStrategy::Checkout.max_sessions(),
            control: super::super::HandoffControl::default(),
            control_reason: None,
            control_since: None,
            handoff: None,
        }
    }
}

impl HubWorker {
    /// The workspace path, when this hub knows it and it is not blank.
    ///
    /// The compatibility shim for everything that read `workspace` as a path
    /// before it grew a type: the hold path resolves a worker by directory, and
    /// the advert places an agent by it.
    pub fn workspace_path(&self) -> Option<&str> {
        self.workspace.as_ref().and_then(|w| w.path())
    }
}
/// The roster shared between the socket layer and the [`HubHandle`].
pub type SharedRoster = Arc<Mutex<Vec<HubWorker>>>;

/// The hosts *this machine* declares, as the advert's `hosts[]` block reads them.
///
/// Shared and appended to rather than snapshotted at launch, for the same reason
/// the roster is: a host started mid-session must be advertised as `local` on the
/// very next registration, and a launch-time copy would describe it as a remote
/// host this hub merely fronts.
///
/// Empty is the honest answer for a hub that hosts nothing itself — every agent
/// it advertises then belongs to somebody else's machine.
pub type SharedLocalHosts = Arc<Mutex<Vec<crate::config::LocalHostRef>>>;

/// Live subscription-selection policy shared by the socket task path and the
/// operator-facing hub handle.
pub type SharedSubscriptionStrategy = Arc<Mutex<crate::runtime::SubscriptionRoutingStrategy>>;
