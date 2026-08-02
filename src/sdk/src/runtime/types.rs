//! Shared data types for the runtime abstraction.
#[allow(unused_imports)]
use super::*;
/// A connected agent medulla can delegate to.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentDescriptor {
    /// Stable roster identity.
    pub id: String,
    /// Model-facing display name.
    #[serde(default)]
    pub name: String,
    /// Prompt-facing description of the agent's role.
    #[serde(default)]
    pub description: String,
    /// Open-vocabulary liveness; only `offline` prevents delegation.
    #[serde(default)]
    pub availability: String,
    /// The workspace this harness-backed agent is deployed into.
    #[serde(
        default,
        rename = "workspaceId",
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_id: Option<String>,
    /// The host for a local agent that has no workspace parent.
    #[serde(default, rename = "hostId", skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    /// Provisioning template provenance, when the agent was spawned from one.
    #[serde(
        default,
        rename = "templateId",
        skip_serializing_if = "Option::is_none"
    )]
    pub template_id: Option<String>,
    /// Model-facing capability tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Opaque harness-owned metadata, never interpreted by this client.
    #[serde(default)]
    pub metadata: Map<String, Value>,
}
/// Latest liveness reading for one roster agent (tinyplace backend only).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentPresence {
    /// Whether the agent is currently considered reachable.
    pub online: bool,
    /// Optional provider- or transport-specific status detail.
    pub detail: Option<String>,
    /// Timestamp of the liveness observation.
    pub at: i64,
}
/// One wrapper session on a peer machine, as shown in the Agents view.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerSession {
    /// Stable wrapper-session identifier.
    pub id: String,
    /// Open-vocabulary session state.
    pub state: String,
    /// Harness serving the session, when reported.
    pub harness: Option<String>,
    /// Timestamp of the most recent observation.
    pub last_seen_at: i64,
}
/// One row in the Chat-tab thread sidebar.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadSummary {
    /// Stable thread identifier.
    pub id: String,
    /// Human-facing thread name.
    pub name: String,
    /// Whether the thread is processing a turn.
    pub running: bool,
    /// Number of recorded turns.
    pub turns: usize,
    /// Number of delegated tasks still running.
    pub running_tasks: usize,
    /// Number of tasks or questions awaiting operator attention.
    pub attention: usize,
}
/// This TUI's own tiny.place identity.
#[derive(Debug, Clone, PartialEq)]
pub struct TinyplaceIdentity {
    /// tiny.place agent identifier for this installation.
    pub agent_id: String,
    /// Public identity key encoded for display or transport setup.
    pub public_key: String,
    /// Optional registered handle.
    pub handle: Option<String>,
}
/// The last cycle's result, as surfaced in the Overview tab.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CycleResultSummary {
    /// Number of orchestration passes consumed.
    pub pass_count: i64,
    /// Final task state keyed by task identifier.
    pub task_ledger: HashMap<String, TaskDigest>,
}
/// One managed worker peer, projected from a `worker.list` entry. A worker is a
/// remote tiny.place peer the orchestrator can delegate to. §4.2 is load-bearing:
/// `id` is the registry's own stable handle (for select/edit/remove), `address` is
/// the messaging target, and `peer_id` (the wallet) is a separate field — never
/// merged.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerInfo {
    /// Stable registry identifier used by worker mutations.
    pub id: String,
    /// Messaging address used to reach the worker.
    pub address: String,
    /// Optional tiny.place handle.
    pub handle: Option<String>,
    /// Optional operator-defined label.
    pub label: Option<String>,
    /// Default coding-agent harness, when known.
    pub harness: Option<String>,
    /// Absolute path this worker runs tasks in, when the hub knows it — the
    /// device-local host reports its own; a remote peer's is unknown until it
    /// answers a capability probe.
    pub workspace: Option<String>,
    /// Wallet or peer identity, distinct from the worker registry identifier.
    pub peer_id: Option<String>,
    /// Logical CPU cores reported by the worker.
    pub cpu_cores: Option<u32>,
    /// Total physical memory reported by the worker, in bytes.
    pub memory_total_bytes: Option<u64>,
    /// Currently available memory reported by the worker, in bytes.
    pub memory_available_bytes: Option<u64>,
    /// Primary IPv4 address reported by the worker.
    pub ip_address: Option<String>,
    /// Whether this worker is the current manual default.
    pub selected: bool,
    /// Agent-template ids this worker is offered for. Empty means unspecified —
    /// a general worker, not one excluded from everything.
    pub roles: Vec<String>,
    /// Per-harness token budgets the worker advertised on its capability probe.
    /// Empty when none were reported. Display-only; the orchestrator sizes tasks.
    pub budgets: Vec<crate::tinyplace::HarnessBudget>,
    /// Per-harness readiness the worker advertised on its capability probe. Empty
    /// when none were reported. Display-only.
    pub readiness: Vec<crate::tinyplace::HarnessReadiness>,
}
/// How the hub chooses a default host from captured capacity details.
///
/// Wire values are camelCase (`manual` / `balanced` / `cpuFirst` / `memoryFirst`),
/// matching the backend's `GET/PUT /medulla/v1/routing/strategy` contract and the
/// persisted `routingStrategy` config key, so one value round-trips across config,
/// backend, and TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RoutingStrategy {
    /// Preserve the operator's explicit host selection.
    Manual,
    /// Prefer CPU, using available memory as the tie-breaker.
    Balanced,
    /// Prefer the worker with the most logical CPU cores.
    CpuFirst,
    /// Prefer the worker with the most currently available memory.
    MemoryFirst,
}

impl RoutingStrategy {
    /// The camelCase wire token (`manual` / `balanced` / `cpuFirst` / `memoryFirst`).
    pub fn as_wire(&self) -> &'static str {
        match self {
            RoutingStrategy::Manual => "manual",
            RoutingStrategy::Balanced => "balanced",
            RoutingStrategy::CpuFirst => "cpuFirst",
            RoutingStrategy::MemoryFirst => "memoryFirst",
        }
    }

    /// Parse a camelCase wire token, or `None` when unrecognized.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "manual" => Some(RoutingStrategy::Manual),
            "balanced" => Some(RoutingStrategy::Balanced),
            "cpuFirst" => Some(RoutingStrategy::CpuFirst),
            "memoryFirst" => Some(RoutingStrategy::MemoryFirst),
            _ => None,
        }
    }

    /// Reconcile a locally-configured strategy with a backend-provided one.
    ///
    /// The backend wins as configuration when it provides one; otherwise the local
    /// config applies; absent both, [`RoutingStrategy::Manual`] preserves the
    /// operator's explicit selection. An operator change is persisted locally by
    /// the caller regardless, so a later backend value overrides the *display* but
    /// never silently discards what the operator saved.
    pub fn reconcile(local: Option<Self>, backend: Option<Self>) -> Self {
        backend.or(local).unwrap_or(RoutingStrategy::Manual)
    }
}
/// How the hub chooses a provider subscription after it has selected a host.
///
/// This is deliberately separate from [`RoutingStrategy`]: CPU and memory
/// describe a host, while token headroom and readiness describe a subscription
/// on that host. Wire values are camelCase for config and future backend parity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubscriptionRoutingStrategy {
    /// Preserve an explicitly requested provider or the host's configured default.
    Manual,
    /// Prefer the ready subscription with the greatest remaining percentage.
    Balanced,
    /// Prefer the ready subscription with the greatest absolute token headroom.
    MostAvailableBudget,
}

impl SubscriptionRoutingStrategy {
    /// The camelCase wire token used by configuration and runtime adapters.
    pub fn as_wire(&self) -> &'static str {
        match self {
            SubscriptionRoutingStrategy::Manual => "manual",
            SubscriptionRoutingStrategy::Balanced => "balanced",
            SubscriptionRoutingStrategy::MostAvailableBudget => "mostAvailableBudget",
        }
    }

    /// Parse a camelCase wire token, or `None` when unrecognized.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "manual" => Some(SubscriptionRoutingStrategy::Manual),
            "balanced" => Some(SubscriptionRoutingStrategy::Balanced),
            "mostAvailableBudget" => Some(SubscriptionRoutingStrategy::MostAvailableBudget),
            _ => None,
        }
    }
}
/// A mutation on the worker-peer registry (`worker.add`/`select`/`update`/`remove`).
#[derive(Debug, Clone)]
pub enum WorkerOp {
    /// Add a worker by address or handle.
    Add {
        /// Direct messaging address, mutually exclusive with `handle`.
        address: Option<String>,
        /// tiny.place handle, mutually exclusive with `address`.
        handle: Option<String>,
        /// Optional operator-facing label.
        label: Option<String>,
        /// Optional default harness hint.
        harness: Option<String>,
    },
    /// Select a worker as the manual default.
    Select {
        /// Registry identifier of the worker.
        id: String,
    },
    /// `patch` is a JSON object of the fields to change (e.g. `{"label": "..."}`); an
    /// empty-string value clears an optional field, mirroring `worker.update`.
    Update {
        /// Registry identifier of the worker.
        id: String,
        /// Partial worker fields to replace.
        patch: Map<String, Value>,
    },
    /// Remove a worker from the registry.
    Remove {
        /// Registry identifier of the worker.
        id: String,
    },
    /// Replace the agent-template roles a worker is offered for.
    ///
    /// Distinct from `Update`, whose patch shape mirrors the backend's
    /// `worker.update` field set. Roles are a whole-list replacement, and
    /// expressing "clear every role" as a patch value is exactly the ambiguity
    /// (`""` clears vs. sets-empty) this avoids.
    SetRoles {
        /// Registry identifier of the worker.
        id: String,
        /// Agent-template ids to offer this worker for; empty leaves it general.
        roles: Vec<String>,
    },
    /// Ask a worker for current CPU, RAM, and IP details.
    RefreshDetails {
        /// Registry identifier of the worker to probe.
        id: String,
    },
    /// Choose the default worker according to captured capacity details.
    ApplyStrategy {
        /// Host-selection strategy to persist and apply.
        strategy: RoutingStrategy,
    },
    /// Choose provider subscriptions independently from host resources.
    ApplySubscriptionStrategy {
        /// Subscription-selection strategy to persist and apply.
        strategy: SubscriptionRoutingStrategy,
    },
}
/// The event stream's health, surfaced in the header when a cycle runs under the
/// core runtime (§01 "lossy-but-not-silently").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    /// `seq` is contiguous; the go-forward tap is trusted.
    Live,
    /// A `seq` gap was seen; a `snapshot.get` rebaselined the folded views.
    Resyncing,
    /// The stream has produced nothing for too long while a cycle is still in flight.
    Stalled,
}
/// One inspected context chunk (`inspect_context`).
#[derive(Debug, Clone, PartialEq)]
pub struct ContextItem {
    /// Stable reference used when citing the context chunk.
    pub ref_: String,
    /// Context source or content kind.
    pub kind: String,
    /// Encoded size of the context chunk.
    pub bytes: usize,
    /// Human-readable context contents.
    pub content: String,
}
/// The full render snapshot (see spec 01 appendix).
#[derive(Debug, Clone, Default)]
pub struct RuntimeSnapshot {
    /// Active main-session identifier.
    pub session_id: String,
    /// Whether a cycle is currently in flight.
    pub running: bool,
    /// Folded orchestration event stream.
    pub events: Vec<EventEnvelope>,
    /// Events projected into the chat surface.
    pub chat_events: Vec<EventEnvelope>,
    /// Materialized chat transcript.
    pub messages: Vec<ChatMessage>,
    /// Summary of the most recently completed cycle.
    pub last_result: Option<CycleResultSummary>,
    /// Whether verbose tracing is enabled.
    pub tracing: bool,
    /// Agents currently known to the runtime.
    pub roster: Vec<AgentDescriptor>,
    /// The declared capacity the roster's agents sit in: hosts, harnesses,
    /// workspaces, and the agent-template catalog. Empty on runtimes that
    /// declare none, which every fleet surface reads as "nothing declared".
    pub capacity: crate::runtime::fleet::CapacitySnapshot,
    /// Latest liveness observation keyed by agent identifier.
    pub presence: HashMap<String, AgentPresence>,
    /// Wrapper sessions grouped by peer identifier.
    pub sessions: HashMap<String, Vec<PeerSession>>,
    /// This installation's tiny.place identity, when configured.
    pub tinyplace: Option<TinyplaceIdentity>,
    /// Resumable chat threads.
    pub threads: Vec<ThreadSummary>,
    /// Thread currently selected for input.
    pub active_thread_id: String,
    /// Latest agent-harness status, when the backing runtime exposes the public
    /// harness contract. `None` until (and unless) the backend surfaces one; the
    /// Agents view renders the compact task board only while it is `Some`.
    pub harness: Option<crate::harness_contract::HarnessStatus>,
    /// Bumped each time the backing runtime rebaselines its folded event log —
    /// e.g. the core runtime clearing state ahead of a reconnect replay — which
    /// restarts the local `seq`s in [`events`](RuntimeSnapshot::events). Pollers
    /// tracking a "last streamed seq" cursor must rewind it when this changes,
    /// or every rebaselined event lands at or below the stale cursor and is
    /// silently dropped. Stays `0` on runtimes that never rebaseline.
    pub replay_epoch: u64,
}
/// The correlation receipt an instruct-style submit returns, when the backing
/// wire carries one (the core runtime's `instruct` `res`, serve-protocol §4.1).
/// It lets a poller tie a later `cycle_end` back to *this* submission instead
/// of completing on the first cycle end it happens to observe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubmitReceipt {
    /// The instruction id the runtime minted for this submission, if reported.
    pub instruction_id: Option<String>,
    /// The cycle id this instruction will run under, if reported.
    pub cycle_id: Option<String>,
}
