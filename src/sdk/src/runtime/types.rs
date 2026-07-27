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
    pub online: bool,
    pub detail: Option<String>,
    pub at: i64,
}
/// One wrapper session on a peer machine, as shown in the Agents view.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerSession {
    pub id: String,
    pub state: String,
    pub harness: Option<String>,
    pub last_seen_at: i64,
}
/// One row in the Chat-tab thread sidebar.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadSummary {
    pub id: String,
    pub name: String,
    pub running: bool,
    pub turns: usize,
    pub running_tasks: usize,
    pub attention: usize,
}
/// This TUI's own tiny.place identity.
#[derive(Debug, Clone, PartialEq)]
pub struct TinyplaceIdentity {
    pub agent_id: String,
    pub public_key: String,
    pub handle: Option<String>,
}
/// The last cycle's result, as surfaced in the Overview tab.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CycleResultSummary {
    pub pass_count: i64,
    pub task_ledger: HashMap<String, TaskDigest>,
}
/// One managed worker peer, projected from a `worker.list` entry. A worker is a
/// remote tiny.place peer the orchestrator can delegate to. §4.2 is load-bearing:
/// `id` is the registry's own stable handle (for select/edit/remove), `address` is
/// the messaging target, and `peer_id` (the wallet) is a separate field — never
/// merged.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerInfo {
    pub id: String,
    pub address: String,
    pub handle: Option<String>,
    pub label: Option<String>,
    pub harness: Option<String>,
    pub peer_id: Option<String>,
    /// Logical CPU cores reported by the worker.
    pub cpu_cores: Option<u32>,
    /// Total physical memory reported by the worker, in bytes.
    pub memory_total_bytes: Option<u64>,
    /// Currently available memory reported by the worker, in bytes.
    pub memory_available_bytes: Option<u64>,
    /// Primary IPv4 address reported by the worker.
    pub ip_address: Option<String>,
    pub selected: bool,
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
    Add {
        address: Option<String>,
        handle: Option<String>,
        label: Option<String>,
        harness: Option<String>,
    },
    Select {
        id: String,
    },
    /// `patch` is a JSON object of the fields to change (e.g. `{"label": "..."}`); an
    /// empty-string value clears an optional field, mirroring `worker.update`.
    Update {
        id: String,
        patch: Map<String, Value>,
    },
    Remove {
        id: String,
    },
    /// Ask a worker for current CPU, RAM, and IP details.
    RefreshDetails {
        id: String,
    },
    /// Choose the default worker according to captured capacity details.
    ApplyStrategy {
        strategy: RoutingStrategy,
    },
    /// Choose provider subscriptions independently from host resources.
    ApplySubscriptionStrategy {
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
    pub ref_: String,
    pub kind: String,
    pub bytes: usize,
    pub content: String,
}
/// The full render snapshot (see spec 01 appendix).
#[derive(Debug, Clone, Default)]
pub struct RuntimeSnapshot {
    pub session_id: String,
    pub running: bool,
    pub events: Vec<EventEnvelope>,
    pub chat_events: Vec<EventEnvelope>,
    pub messages: Vec<ChatMessage>,
    pub last_result: Option<CycleResultSummary>,
    pub tracing: bool,
    pub roster: Vec<AgentDescriptor>,
    /// The declared capacity the roster's agents sit in: hosts, harnesses,
    /// workspaces, and the agent-template catalog. Empty on runtimes that
    /// declare none, which every fleet surface reads as "nothing declared".
    pub capacity: crate::runtime::fleet::CapacitySnapshot,
    pub presence: HashMap<String, AgentPresence>,
    pub sessions: HashMap<String, Vec<PeerSession>>,
    pub tinyplace: Option<TinyplaceIdentity>,
    pub threads: Vec<ThreadSummary>,
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
