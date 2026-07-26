//! Project the backend's connected-worker roster onto the declared-capacity
//! containment chain the fleet surfaces render.
//!
//! `GET /medulla/v1/roster` answers in the manager's own vocabulary: one flat
//! [`RosterWorker`] row per connected worker, carrying the machine's resources,
//! the harness serving it, and its budget windows all on the same record. The UI
//! reads the medulla-v1 chain instead — `Host → Harness → Workspace → Agent` —
//! so this module splits each row back into the levels it flattened.
//!
//! The split is deliberately conservative. A worker names exactly one machine
//! and one harness, so each row yields one host and one harness of its own
//! rather than being coalesced with a look-alike: two workers on one physical
//! box are still two rows to the manager, and inventing a shared parent would
//! claim a fact the wire never stated. A workspace is only synthesized when the
//! worker's capability probe actually reported a working directory; without one
//! the agent hangs off its host directly, exactly as a local agent does.

use serde_json::{Map, Value};

use crate::client::{RosterBudget, RosterWorker};
use crate::runtime::fleet::{
    CapacitySnapshot, HarnessBudget, HarnessDescriptor, HostDescriptor, HostResources,
    WorkspaceDescriptor,
};
use crate::runtime::AgentDescriptor;

/// The roster and capacity a list of connected workers projects onto.
///
/// Returned together because they are one read of one endpoint: the agents are
/// the chain's leaf level and the capacity is everything above it, and letting
/// them drift apart is how an agent ends up pointing at a workspace no snapshot
/// declares.
pub(super) fn project_roster(workers: &[RosterWorker]) -> (Vec<AgentDescriptor>, CapacitySnapshot) {
    let mut agents = Vec::with_capacity(workers.len());
    let mut capacity = CapacitySnapshot::default();

    for worker in workers {
        let host_id = format!("host:{}", worker.registry_id);
        let harness_id = format!("harness:{}", worker.registry_id);

        capacity.hosts.push(HostDescriptor {
            id: host_id.clone(),
            name: display_name(worker),
            availability: worker.availability.clone(),
            address: worker
                .primary_ipv4
                .clone()
                .or_else(|| worker.address.clone()),
            resources: host_resources(worker),
            metadata: host_metadata(worker),
        });

        capacity.harnesses.push(HarnessDescriptor {
            id: harness_id.clone(),
            host_id: host_id.clone(),
            kind: worker
                .harness
                .clone()
                .filter(|h| !h.trim().is_empty())
                .unwrap_or_else(|| "unknown".into()),
            availability: worker.availability.clone(),
            // The roster has no declared readiness of its own, so availability
            // is the only signal: anything the manager has not marked offline is
            // treated as able to take work, matching auto-assign's hard gate.
            ready: !worker.availability.eq_ignore_ascii_case("offline"),
            ready_reason: None,
            providers: worker.budgets.iter().map(|b| b.provider.clone()).collect(),
            template_ids: Vec::new(),
            budgets: worker.budgets.iter().map(budget).collect(),
            metadata: Map::new(),
        });

        // Only a probed cwd justifies a workspace row; a worker that never
        // reported one is not deployed into a folder we can name.
        let workspace_id = probed_cwd(worker).map(|cwd| {
            let id = format!("ws:{}", worker.registry_id);
            capacity.workspaces.push(WorkspaceDescriptor {
                id: id.clone(),
                name: probed_str(worker, "project")
                    .or_else(|| cwd.rsplit('/').next().map(str::to_string))
                    .unwrap_or_else(|| cwd.clone()),
                path: cwd,
                harness_id: harness_id.clone(),
                profile: None,
                project: probed_str(worker, "project"),
                template_ids: Vec::new(),
                metadata: probed_str(worker, "branch")
                    .map(|b| {
                        let mut m = Map::new();
                        m.insert("branch".into(), Value::String(b));
                        m
                    })
                    .unwrap_or_default(),
            });
            id
        });

        agents.push(AgentDescriptor {
            id: worker.registry_id.clone(),
            name: display_name(worker),
            description: worker.description.clone(),
            availability: worker.availability.clone(),
            // A workspace-backed agent derives its host by walking up; only an
            // agent without one may name a host directly, so exactly one of
            // these is ever set.
            host_id: workspace_id.is_none().then(|| host_id.clone()),
            workspace_id,
            template_id: None,
            tags: worker.tags.clone(),
            metadata: agent_metadata(worker),
        });
    }

    (agents, capacity)
}

/// The label a worker shows under, falling back through handle and id so a row
/// is never nameless.
fn display_name(worker: &RosterWorker) -> String {
    [
        worker.label.as_str(),
        worker.handle.as_deref().unwrap_or_default(),
        worker.registry_id.as_str(),
    ]
    .into_iter()
    .map(str::trim)
    .find(|s| !s.is_empty())
    .unwrap_or_default()
    .to_string()
}

/// The machine-level facts flattened onto the roster row, or `None` when the
/// worker reported none of them.
fn host_resources(worker: &RosterWorker) -> Option<HostResources> {
    let resources = HostResources {
        cpu_cores: worker.cpu_cores.map(f64::from),
        total_memory_bytes: worker.total_mem_bytes,
        available_memory_bytes: worker.available_mem_bytes,
        disk_free_bytes: None,
        ..Default::default()
    };
    (resources != HostResources::default()).then_some(resources)
}

/// Reachability facts worth showing on the host row but not modelled as fields.
fn host_metadata(worker: &RosterWorker) -> Map<String, Value> {
    let mut meta = Map::new();
    if let Some(ip) = worker.primary_ipv4.as_deref().filter(|s| !s.is_empty()) {
        meta.insert("primaryIpv4".into(), Value::String(ip.into()));
    }
    if let Some(peer) = worker.wallet_peer_id.as_deref().filter(|s| !s.is_empty()) {
        meta.insert("walletPeerId".into(), Value::String(peer.into()));
    }
    meta
}

/// The identity/routing facts the Agents view already reads off `metadata`.
fn agent_metadata(worker: &RosterWorker) -> Map<String, Value> {
    let mut meta = Map::new();
    if let Some(harness) = worker.harness.as_deref().filter(|s| !s.trim().is_empty()) {
        meta.insert("harness".into(), Value::String(harness.into()));
    }
    if let Some(address) = worker.address.as_deref().filter(|s| !s.is_empty()) {
        meta.insert("address".into(), Value::String(address.into()));
    }
    if let Some(handle) = worker.handle.as_deref().filter(|s| !s.is_empty()) {
        meta.insert("handle".into(), Value::String(handle.into()));
    }
    meta.insert("selected".into(), Value::Bool(worker.selected));
    meta
}

/// One client-safe roster budget as the chain's harness budget.
fn budget(source: &RosterBudget) -> HarnessBudget {
    HarnessBudget {
        provider: source.provider.clone(),
        window: source.window.clone(),
        seat: source.seat.clone(),
        account_type: source.account_type.clone(),
        limit_tokens: source.limit_tokens,
        used_tokens: None,
        remaining_tokens: source.remaining_tokens,
        amount_unit: source.amount_unit.clone(),
        limit_amount: source.limit_amount,
        used_amount: source.used_amount,
        remaining_amount: source.remaining_amount,
        cooldown_until: source.cooldown_until,
        source: source.source.clone(),
    }
}

/// A string field from the worker's opaque capability document.
fn probed_str(worker: &RosterWorker, key: &str) -> Option<String> {
    worker
        .capabilities
        .as_ref()?
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The working directory the capability probe reported, if any.
fn probed_cwd(worker: &RosterWorker) -> Option<String> {
    probed_str(worker, "cwd")
}
