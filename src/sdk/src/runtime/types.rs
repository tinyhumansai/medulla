//! Shared data types for the runtime abstraction.
#[allow(unused_imports)]
use super::*;
/// A connected agent medulla can delegate to.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentDescriptor {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub availability: String,
    #[serde(default)]
    pub tags: Vec<String>,
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
    pub parent_id: Option<String>,
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
/// How the hub chooses a default worker from captured capacity details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingStrategy {
    /// Preserve the operator's explicit worker selection.
    Manual,
    /// Prefer CPU, using available memory as the tie-breaker.
    Balanced,
    /// Prefer the worker with the most logical CPU cores.
    CpuFirst,
    /// Prefer the worker with the most currently available memory.
    MemoryFirst,
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
    pub presence: HashMap<String, AgentPresence>,
    pub sessions: HashMap<String, Vec<PeerSession>>,
    pub tinyplace: Option<TinyplaceIdentity>,
    pub async_mode: bool,
    pub threads: Vec<ThreadSummary>,
    pub active_thread_id: String,
    /// Latest agent-harness status, when the backing runtime fronts a medulla-v1
    /// agent harness. `None` until (and unless) the backend surfaces one; the
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
