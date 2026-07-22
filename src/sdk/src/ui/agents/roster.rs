//! Merge the local worker registry into the Agents-view roster.
//!
//! [`derive_agent_lanes`](super::derive_agent_lanes) seeds one lane per
//! [`AgentDescriptor`] in the render snapshot, and that roster only ever carries
//! what a *backend* advertises. A tiny.place worker added at runtime lives in
//! the orchestrator hub's own roster instead — the very list that resolves a
//! delegated task's address — so it was reachable, dispatchable, and completely
//! absent from the Agents tab until a task happened to be running on it.
//!
//! This module projects the [`WorkerInfo`] rows that registry surfaces onto
//! descriptors and merges them into the snapshot roster, so the Agents and
//! Workers tabs describe one fleet rather than two.

use serde_json::{Map, Value};

use crate::runtime::{AgentDescriptor, WorkerInfo};

/// The metadata key marking a descriptor that came from the local registry
/// rather than the backend roster.
const SOURCE_LOCAL: &str = "local";

/// Whether `descriptor` already names the peer `worker` names.
///
/// Matched on address as well as id, mirroring the hub's own de-duplication:
/// one peer is routinely known by two names (the id a pre-seeded
/// `MEDULLA_HUB_WORKERS` entry gave it, the address the Workers tab used, the
/// cryptoId behind an `@handle`), and listing it twice would show one worker as
/// two lanes. Blank never matches — two peers with no address are not the same
/// peer.
fn names_same_peer(descriptor: &AgentDescriptor, worker: &WorkerInfo) -> bool {
    let known: [&str; 2] = [
        descriptor.id.as_str(),
        descriptor
            .metadata
            .get("address")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    ];
    let wanted: [&str; 2] = [worker.id.as_str(), worker.address.as_str()];
    known.iter().any(|k| {
        let k = k.trim();
        !k.is_empty() && wanted.iter().any(|w| w.trim() == k)
    })
}

/// The descriptor a registry worker shows as in the Agents view.
///
/// Deliberately claims nothing it does not know: `availability` stays empty
/// (the registry tracks reachability, not liveness) so the lane renders as
/// "announced, no reading" rather than as an online agent. The harness lands in
/// `metadata` because that is what tags the lane label, and the address because
/// that is what identifies the peer when a label is absent.
pub fn worker_descriptor(worker: &WorkerInfo) -> AgentDescriptor {
    let mut metadata = Map::new();
    metadata.insert("source".into(), Value::String(SOURCE_LOCAL.into()));
    if !worker.address.trim().is_empty() {
        metadata.insert("address".into(), Value::String(worker.address.clone()));
    }
    if let Some(handle) = &worker.handle {
        metadata.insert("handle".into(), Value::String(handle.clone()));
    }
    if let Some(harness) = worker.harness.as_deref().filter(|h| !h.trim().is_empty()) {
        metadata.insert("harness".into(), Value::String(harness.to_string()));
    }
    let name = [
        worker.label.as_deref(),
        worker.handle.as_deref(),
        Some(worker.address.as_str()),
        Some(worker.id.as_str()),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|s| !s.is_empty())
    .unwrap_or_default()
    .to_string();
    AgentDescriptor {
        id: worker.id.clone(),
        name,
        description: worker
            .harness
            .as_deref()
            .filter(|h| !h.trim().is_empty())
            .map(|h| format!("{h} daemon"))
            .unwrap_or_default(),
        availability: String::new(),
        tags: Vec::new(),
        metadata,
    }
}

/// `roster` followed by every registry worker it does not already describe.
///
/// The snapshot roster wins on a collision: a peer the backend advertises
/// carries capabilities and availability the local registry entry does not.
pub fn merge_worker_roster(
    roster: &[AgentDescriptor],
    workers: &[WorkerInfo],
) -> Vec<AgentDescriptor> {
    let mut out = roster.to_vec();
    for worker in workers {
        if out.iter().any(|d| names_same_peer(d, worker)) {
            continue;
        }
        out.push(worker_descriptor(worker));
    }
    out
}
