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
///
/// `availability` is `"online"`, not blank. The orchestrator only auto-assigns
/// an untargeted task to an agent whose availability is exactly `"online"`, so a
/// blank one is silently excluded from every fan-out — and it renders as an
/// empty column in `agent_list`, which reads as a broken row rather than an
/// idle worker. A roster entry exists because an operator put it there; genuine
/// unreachability surfaces as a task error, which is honest, whereas advertising
/// "offline" would refuse delegation outright.
fn to_agent(w: &HubWorker) -> Value {
    json!({
        "id": w.id,
        // The name falls back to the id, not to a second constant. `agent_list`
        // renders `id (name)`, so two different readable tokens put the wrong
        // answer back on the table — which is the whole failure being fixed
        // here. Unlabelled, the two coincide and there is nothing to get wrong;
        // labelled, the id is a visible slug of the name.
        "name": w.label.clone().unwrap_or_else(|| w.id.clone()),
        "description": format!("{} daemon", w.harness),
        "availability": "online",
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
/// Two cases that used to be one. An **absent** `agentId` means "any worker" —
/// the backend omits it for an unattributed task — and falls back to the
/// selected entry, then the first. An `agentId` that is *present but
/// unrecognised* is a different thing entirely: something addressed a specific
/// agent this hub does not have. Falling back there silently ran the work on
/// whichever worker happened to be first, which is a wrong answer wearing the
/// costume of a right one.
///
/// Matched on address as well as id so a roster saved before ids were
/// human-scale — where the id *was* the cryptoId — keeps resolving.
pub(super) fn address_of(workers: &[HubWorker], agent_id: &str) -> Option<String> {
    let wanted = agent_id.trim();
    if wanted.is_empty() {
        return workers
            .iter()
            .find(|w| w.selected)
            .or_else(|| workers.first())
            .map(|w| w.address.clone());
    }
    workers
        .iter()
        .find(|w| w.id == wanted || w.address == wanted)
        .map(|w| w.address.clone())
}

/// Whether two roster entries name the same destination.
///
/// Blank never matches: an entry with no address is not "the same peer" as
/// another entry with no address.
pub(super) fn same_destination(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    !a.is_empty() && a == b
}

/// Drop every entry that names the same worker as `incoming`.
///
/// Matched on **address as well as id**, because the address is the peer's
/// wallet and the actual delegation target. Two entries differing only in id are
/// two names for one destination: the backend would be advertised the same
/// worker twice, and [`address_of`] could resolve a task to either of them.
///
/// Ids diverge easily in practice — `MEDULLA_HUB_WORKERS="alpha=<addr>"` seeds
/// `alpha`, while adding the same address in the TUI uses the address itself,
/// and an `@handle` differs from the cryptoId it resolves to.
pub(super) fn remove_conflicting(workers: &mut Vec<HubWorker>, incoming: &HubWorker) {
    workers.retain(|w| w.id != incoming.id && !same_destination(&w.address, &incoming.address));
}

/// A short, stable, human-scale id for a worker.
///
/// The id is what the orchestrator must reproduce to address this worker: it is
/// rendered first in `agent_list` (`id (name)`) and copied into a task's
/// `agentId`. A 44-character base58 cryptoId reads as noise beside a memorable
/// name, and the model reaches for the name — which then fails validation as an
/// unknown agent. Making the id the memorable token removes the wrong answer
/// instead of catching it.
///
/// The cryptoId is not lost: it stays the `address`, and is advertised in the
/// descriptor's metadata.
pub(crate) fn worker_id(label: Option<&str>, harness: &str, taken: &[String]) -> String {
    let base = label
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(slug)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}-worker", slug(harness)));
    let base = if base.is_empty() {
        "worker".to_string()
    } else {
        base
    };
    if !taken.iter().any(|t| t == &base) {
        return base;
    }
    // Two workers on one harness with no labels is ordinary; ids must still be
    // distinct or one would shadow the other in the backend's registry.
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken.iter().any(|t| t == candidate))
        .expect("an unbounded search always terminates")
}

/// Lowercase, hyphen-separated, alphanumeric — safe to type and to round-trip.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}
