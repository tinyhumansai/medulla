//! The hub's worker-roster data: the shared roster type, the `AgentDescriptor`
//! payload the hub advertises, and the address resolution the socket layer uses
//! to target a task. Pure and offline-testable; the live control handle that
//! mutates the roster over the Socket.IO uplink lives in [`handle`](super::handle).
//!
//! The roster is shared (`Arc<Mutex<_>>`) between the Socket.IO layer — which
//! reads it to advertise agents and resolve a task's address — and the
//! [`HubHandle`](super::handle::HubHandle) the TUI holds to add/remove workers at
//! runtime. Every mutation re-emits `medulla:register_agents` so the backend's
//! roster tracks the change.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

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
}

/// The roster shared between the socket layer and the [`HubHandle`].
pub type SharedRoster = Arc<Mutex<Vec<HubWorker>>>;

/// The `AgentDescriptor` a single worker advertises.
fn to_agent(w: &HubWorker) -> Value {
    json!({
        "id": w.id,
        "name": w.label.clone().unwrap_or_else(|| "tinyplace-worker".to_string()),
        "description": format!("{} daemon", w.harness),
        "availability": "",
        "tags": ["code"],
        "metadata": { "address": w.address, "harness": w.harness },
    })
}

/// The `register_agents` payload for the current roster.
pub(super) fn register_payload(workers: &[HubWorker]) -> Value {
    json!({ "agents": workers.iter().map(to_agent).collect::<Vec<_>>() })
}

/// Resolve a targeted `agentId` to a tiny.place address.
///
/// Prefers the roster entry whose id matches. If the id is unknown (or the
/// backend omitted `agentId` entirely — an empty string), falls back to the
/// **selected** worker, then the first — never the empty/unknown id, which would
/// decode to a zero-length key and fail the send. `None` only when the roster is
/// empty.
pub(super) fn address_of(workers: &[HubWorker], agent_id: &str) -> Option<String> {
    workers
        .iter()
        .find(|w| w.id == agent_id)
        .or_else(|| workers.iter().find(|w| w.selected))
        .or_else(|| workers.first())
        .map(|w| w.address.clone())
}
