//! Project the local peer registry onto the containment chain.
//!
//! The registry — what `Runtime::workers()` returns, and what the Hosts page
//! manages — predates the chain and keeps the wire's old name. What it holds is
//! the host level: a machine, reachable over tiny.place, advertising the agent
//! CLI runtimes it has, the cores and memory it runs on, and the token budgets
//! those runtimes report.
//!
//! So a registered peer becomes a host plus one harness per provider it
//! declared, rather than an "agent" with nowhere to live. Without this it landed
//! in the Fleet page's `unplaced agents` group — technically true of the old
//! vocabulary, and wrong about what the thing actually is.

use serde_json::{Map, Value};

use crate::runtime::fleet::{
    CapacitySnapshot, HarnessBudget, HarnessDescriptor, HostDescriptor, HostResources,
};
use crate::runtime::WorkerInfo;

/// The hosts and harnesses a local registry declares.
pub fn registry_capacity(peers: &[WorkerInfo]) -> CapacitySnapshot {
    let mut capacity = CapacitySnapshot::default();
    for peer in peers {
        let host_id = format!("host:{}", peer.id);
        capacity.hosts.push(HostDescriptor {
            id: host_id.clone(),
            name: display_name(peer),
            // The registry tracks reachability, not liveness, so it must not
            // claim a machine is online. Blank reads as "announced, no reading"
            // and never trips the offline styling.
            availability: String::new(),
            address: peer.ip_address.clone().or(Some(peer.address.clone())),
            resources: resources(peer),
            metadata: metadata(peer),
        });
        capacity.harnesses.extend(harnesses(peer, &host_id));
    }
    capacity
}

/// The harnesses one peer declares: one per provider it reported readiness or a
/// budget for, falling back to the single `harness` label it registered with.
fn harnesses(peer: &WorkerInfo, host_id: &str) -> Vec<HarnessDescriptor> {
    let mut kinds: Vec<String> = Vec::new();
    for readiness in &peer.readiness {
        push_unique(&mut kinds, readiness.provider.as_str().to_string());
    }
    for budget in &peer.budgets {
        push_unique(&mut kinds, budget.provider.as_str().to_string());
    }
    if kinds.is_empty() {
        if let Some(harness) = peer.harness.as_deref().filter(|h| !h.trim().is_empty()) {
            kinds.push(harness.to_string());
        }
    }

    kinds
        .into_iter()
        .map(|kind| {
            let readiness = peer.readiness.iter().find(|r| r.provider.as_str() == kind);
            HarnessDescriptor {
                id: format!("harness:{}:{kind}", peer.id),
                host_id: host_id.to_string(),
                kind: kind.clone(),
                availability: String::new(),
                // No probe result means no evidence either way; the fleet reads
                // fail-open, so an unprobed runtime is shown as usable.
                ready: readiness.map(|r| r.ready).unwrap_or(true),
                ready_reason: readiness.and_then(|r| r.reason.clone()),
                providers: vec![kind.clone()],
                template_ids: Vec::new(),
                budgets: peer
                    .budgets
                    .iter()
                    .filter(|b| b.provider.as_str() == kind)
                    .map(budget)
                    .collect(),
                metadata: Map::new(),
            }
        })
        .collect()
}

/// One advertised harness budget as the chain's declared form.
fn budget(source: &crate::tinyplace::HarnessBudget) -> HarnessBudget {
    HarnessBudget {
        provider: source.provider.as_str().to_string(),
        window: window(source.window),
        seat: source.seat.clone(),
        limit_tokens: source.limit_tokens.and_then(|v| u64::try_from(v).ok()),
        used_tokens: source.used_tokens.and_then(|v| u64::try_from(v).ok()),
        remaining_tokens: source.remaining_tokens.and_then(|v| u64::try_from(v).ok()),
        cooldown_until: source.cooldown_until.and_then(|v| u64::try_from(v).ok()),
        source: budget_source(source.source),
    }
}

/// A metering window in the chain's open-vocabulary string form.
fn window(window: crate::tinyplace::BudgetWindow) -> String {
    use crate::tinyplace::BudgetWindow as W;
    match window {
        W::Daily => "daily",
        W::Weekly => "weekly",
        W::FiveHour => "5h",
        W::Unknown => "window unknown",
    }
    .to_string()
}

/// How a budget's numbers were arrived at, in the chain's string form.
fn budget_source(source: crate::tinyplace::BudgetSource) -> String {
    use crate::tinyplace::BudgetSource as S;
    match source {
        S::Estimate => "estimate",
        S::ProviderReported => "provider_reported",
        S::Configured => "configured",
    }
    .to_string()
}

/// The label a peer shows under, falling back through handle and address.
fn display_name(peer: &WorkerInfo) -> String {
    [
        peer.label.as_deref().unwrap_or_default(),
        peer.handle.as_deref().unwrap_or_default(),
        peer.address.as_str(),
        peer.id.as_str(),
    ]
    .into_iter()
    .map(str::trim)
    .find(|s| !s.is_empty())
    .unwrap_or_default()
    .to_string()
}

/// The machine facts the peer reported, or `None` when it reported none.
fn resources(peer: &WorkerInfo) -> Option<HostResources> {
    let resources = HostResources {
        cpu_cores: peer.cpu_cores.map(f64::from),
        total_memory_bytes: peer.memory_total_bytes,
        available_memory_bytes: peer.memory_available_bytes,
        disk_free_bytes: None,
    };
    (resources != HostResources::default()).then_some(resources)
}

/// Identity worth showing on the host row but not modelled as a field.
fn metadata(peer: &WorkerInfo) -> Map<String, Value> {
    let mut meta = Map::new();
    meta.insert("address".into(), Value::String(peer.address.clone()));
    if let Some(handle) = peer.handle.as_deref().filter(|s| !s.is_empty()) {
        meta.insert("handle".into(), Value::String(handle.into()));
    }
    if let Some(peer_id) = peer.peer_id.as_deref().filter(|s| !s.is_empty()) {
        meta.insert("peerId".into(), Value::String(peer_id.into()));
    }
    meta.insert("selected".into(), Value::Bool(peer.selected));
    meta
}

/// Push `value` when the list does not already hold it.
fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

/// `base` plus every host and harness of `extra` it does not already describe.
///
/// The runtime's own declaration wins on a collision — it carries availability
/// and workspaces the registry never sees — matched on host id and on address,
/// mirroring the roster merge: one machine is routinely known by two names, and
/// listing it twice would show one host as two.
pub fn merge_capacity(base: &CapacitySnapshot, extra: &CapacitySnapshot) -> CapacitySnapshot {
    let mut out = base.clone();
    for host in &extra.hosts {
        if out.hosts.iter().any(|existing| same_host(existing, host)) {
            continue;
        }
        out.hosts.push(host.clone());
        out.harnesses.extend(
            extra
                .harnesses
                .iter()
                .filter(|h| h.host_id == host.id)
                .cloned(),
        );
    }
    out
}

/// Whether two host records name the same machine.
fn same_host(a: &HostDescriptor, b: &HostDescriptor) -> bool {
    if a.id == b.id {
        return true;
    }
    match (a.address.as_deref(), b.address.as_deref()) {
        (Some(x), Some(y)) => !x.trim().is_empty() && x == y,
        _ => false,
    }
}
