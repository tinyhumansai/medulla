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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

/// The `AgentDescriptor` a single worker advertises.
///
/// Only ever built for a worker that survived the liveness filter, so
/// `availability` is always `"online"` here. It stays on the wire because the
/// orchestrator only auto-assigns an untargeted task to an agent whose
/// availability is exactly `"online"`, and a blank one would be silently
/// excluded from every fan-out.
fn to_agent(w: &HubWorker, catalog: &[crate::runtime::AgentTemplate]) -> Value {
    // The roles this worker was toggled on for, resolved against the catalog.
    // An id with no template behind it is dropped rather than advertised: a
    // role the orchestrator cannot look up is a routing hint it cannot act on.
    let roles: Vec<&crate::runtime::AgentTemplate> = w
        .roles
        .iter()
        .filter_map(|id| catalog.iter().find(|t| &t.id == id))
        .collect();
    // `metadata.workspace` is what places the agent: the backend turns it into a
    // WorkspaceDescriptor and sets the agent's `workspaceId` from it. Omitted
    // rather than sent empty when unknown, so the backend falls through to the
    // worker's probed `capabilities.cwd` instead of placing it at "".
    let mut metadata = json!({ "address": w.address, "harness": w.harness });
    // The role ids, not just their text. The description and tags are what the
    // model reads; the ids are what anything downstream joins on — asking for a
    // role by name, or applying its tools and instructions at delegate time.
    //
    // The *resolved* ids, for that reason. Advertising an id the catalog does
    // not have hands a joiner a key with nothing behind it, which is the same
    // unactionable hint the description and tags already drop.
    if !roles.is_empty() {
        metadata["roles"] = json!(roles.iter().map(|t| t.id.as_str()).collect::<Vec<_>>());
    }
    // The path, not the `{path, type}` object the entity model carries: the
    // object is the wire change, and this advert stays byte-identical for a
    // worker whose placement has not changed.
    if let Some(workspace) = w.workspace_path() {
        metadata["workspace"] = json!(workspace);
    }
    // Who holds the harness, and only when that is a person. Absent means the
    // orchestrator has it, which is both the common case and the one worth
    // keeping byte-stable: this advert is re-emitted on every roster mutation,
    // and a key that flips on each one is a diff nobody can read.
    if w.control.is_operator() {
        metadata["control"] = json!(w.control.as_str());
        if let Some(reason) = w
            .control_reason
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        {
            metadata["controlReason"] = json!(reason);
        }
        if let Some(since) = w.control_since {
            metadata["controlSince"] = json!(since);
        }
    }
    // The brief from the last handback. Carried only while the orchestrator
    // actually holds the harness: an invitation to continue work in a workspace
    // the operator has since re-taken is one the orchestrator cannot act on, and
    // planning against it wastes a pass.
    if let (false, Some(handoff)) = (w.control.is_operator(), w.handoff.as_ref()) {
        if let Ok(value) = serde_json::to_value(handoff) {
            metadata["handoff"] = value;
        }
    }
    json!({
        "id": w.id,
        // The name falls back to the id, not to a second constant. `agent_list`
        // renders `id (name)`, so two different readable tokens put the wrong
        // answer back on the table — which is the whole failure being fixed
        // here. Unlabelled, the two coincide and there is nothing to get wrong;
        // labelled, the id is a visible slug of the name.
        "name": w.label.clone().unwrap_or_else(|| w.id.clone()),
        // What this worker is *for*, when the operator has said. The harness is
        // how work runs; the role is what work it should get, and only the
        // second is something the orchestrator can route on. With no roles
        // chosen this is the harness line it always was — an unspecified worker
        // stays a general one rather than being described as nothing.
        "description": if roles.is_empty() {
            format!("{} daemon", w.harness)
        } else {
            roles
                .iter()
                .map(|t| t.description.trim())
                .filter(|d| !d.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        },
        "availability": "online",
        // `code` is kept whatever else is set: every one of these runs a coding
        // harness, and dropping it would take a role-tagged worker out of the
        // fan-outs that ask for code.
        "tags": role_tags(&roles),
        "metadata": metadata,
    })
}

/// The tag set a worker advertises: `code`, plus each role's own tags.
///
/// Deduplicated and ordered, so two roles sharing a tag advertise it once and
/// the list does not reshuffle between registrations for no reason.
fn role_tags(roles: &[&crate::runtime::AgentTemplate]) -> Vec<String> {
    let mut tags = vec!["code".to_string()];
    for tag in roles.iter().flat_map(|t| t.tags.iter()) {
        let tag = tag.trim();
        if !tag.is_empty() && !tags.iter().any(|held| held == tag) {
            tags.push(tag.to_string());
        }
    }
    tags
}

/// The roster entry a [`WorkerSpec`](crate::hub::WorkerSpec) describes.
///
/// The one place a spec becomes a live roster row, so the declaration → advert
/// chain has a single seam: an agent's `roles`, workspace and derived capacity
/// arrive here from what the operator declared, and `to_agent` turns them into
/// `metadata`. Before declarations existed this mapping hard-coded empty roles,
/// which is why a host in this process advertised itself as a general worker
/// however it was configured.
///
/// Lives here rather than beside the hub's boot wiring because it is pure data
/// translation, and because everything downstream of it is tested here.
pub(crate) fn worker_from_spec(spec: &crate::hub::WorkerSpec) -> HubWorker {
    HubWorker {
        id: spec.id.clone(),
        host_id: spec.host_id.clone(),
        address: spec.address.clone(),
        harness: spec.harness.clone(),
        // The placeholder name is not a label. It is what an env-seeded or
        // remembered row carries when nobody named it, and promoting it to a
        // label would put a constant on screen where the id belongs.
        label: (spec.name != "medulla-worker").then(|| spec.name.clone()),
        selected: false,
        roles: spec.roles.clone(),
        workspace: spec.workspace.clone(),
        // A spec that states no capacity is a remembered roster row or a remote
        // peer, not a declaration; the serial default is the safe reading.
        max_sessions: if spec.max_sessions == 0 {
            crate::runtime::WorkspaceStrategy::Checkout.max_sessions()
        } else {
            spec.max_sessions
        },
        ..Default::default()
    }
}

/// The `register_agents` payload for the roster, minus anything known to be down.
///
/// `online` maps a worker address to its liveness, as
/// [`Bridge::presence`](crate::bridge::Bridge::presence) reported it. An empty
/// map is "no opinion" and advertises everything.
///
/// Withheld rather than advertised as offline. Marking one down only stops the
/// *automatic* assignment of an untargeted task — a task that names a specific
/// agent still resolves through [`address_of`] and dispatches into the void,
/// which is the stall this is meant to prevent. `agent_list` is also what the
/// orchestrator reads to decide who exists at all, and an agent it cannot reach
/// is not a choice worth offering it.
///
/// An address the query had no answer for is kept. "The relay did not say" is
/// not "the worker is down", and one dropped request must not empty a live
/// roster.
pub(super) fn register_payload(
    workers: &[HubWorker],
    online: &std::collections::HashMap<String, bool>,
    catalog: &[crate::runtime::AgentTemplate],
) -> Value {
    let reachable = workers.iter().filter(|w| is_reachable(w, online));
    json!({
        "agents": reachable
            .map(|w| to_agent(w, catalog))
            .collect::<Vec<_>>()
    })
}

/// Whether `w` should be advertised, given what presence reported.
///
/// Only an explicit `false` withholds a worker; both "reported up" and "no
/// answer" advertise it.
pub(super) fn is_reachable(
    w: &HubWorker,
    online: &std::collections::HashMap<String, bool>,
) -> bool {
    online.get(&w.address) != Some(&false)
}

/// The addresses in `workers` the presence query reported down.
pub(super) fn unreachable_addresses(
    workers: &[HubWorker],
    online: &std::collections::HashMap<String, bool>,
) -> Vec<String> {
    workers
        .iter()
        .filter(|w| !is_reachable(w, online))
        .map(|w| w.address.clone())
        .collect()
}

/// The addresses in `workers`, for a batched presence query.
pub(super) fn addresses_of(workers: &[HubWorker]) -> Vec<String> {
    workers.iter().map(|w| w.address.clone()).collect()
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

/// The roster id a dispatch is grouped under — the lane the Agents view files
/// its task in.
///
/// The targeted agent when the task named one, and only otherwise the first
/// entry at the resolved `address`. The order matters now that a machine
/// advertises several agents on one address: resolving by address alone files
/// every task on that machine under whichever agent happens to be listed first,
/// so work dispatched to `this-device-codex` would appear under `this-device`.
///
/// Falls back rather than failing, because a lane label is not worth dropping a
/// task over: an unattributed dispatch has no id to prefer, and an unknown one
/// has already been refused by [`address_of`].
pub(super) fn lane_id(workers: &[HubWorker], agent_id: &str, address: Option<&str>) -> String {
    let wanted = agent_id.trim();
    if !wanted.is_empty() {
        if let Some(worker) = workers
            .iter()
            .find(|w| w.id == wanted || w.address == wanted)
        {
            return worker.id.clone();
        }
    }
    address
        .and_then(|address| workers.iter().find(|w| w.address == address))
        .map(|w| w.id.clone())
        .unwrap_or_default()
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
///
/// Returns the removed ids so callers can invalidate state cached by roster id.
pub(super) fn remove_conflicting(
    workers: &mut Vec<HubWorker>,
    incoming: &HubWorker,
) -> Vec<String> {
    let mut removed_ids = Vec::new();
    workers.retain(|worker| {
        let conflicts =
            worker.id == incoming.id || same_destination(&worker.address, &incoming.address);
        if conflicts {
            removed_ids.push(worker.id.clone());
        }
        !conflicts
    });
    removed_ids
}

/// Choose a worker id from captured capacity details.
///
/// Manual routing deliberately returns `None`: preserving the current selection
/// is an action in itself, not an automatic re-selection.
pub(super) fn worker_for_strategy(
    workers: &[HubWorker],
    details: &HashMap<String, crate::protocol::WorkerSystemInfo>,
    strategy: crate::runtime::RoutingStrategy,
) -> Option<String> {
    use crate::runtime::RoutingStrategy;

    if strategy == RoutingStrategy::Manual {
        return None;
    }
    workers
        .iter()
        .filter_map(|worker| {
            let info = details.get(&worker.id)?;
            let memory = info.memory_available_bytes.unwrap_or(0);
            let key = match strategy {
                RoutingStrategy::Balanced => (info.cpu_cores as u64, memory),
                RoutingStrategy::CpuFirst => (info.cpu_cores as u64, 0),
                RoutingStrategy::MemoryFirst => (memory, info.cpu_cores as u64),
                RoutingStrategy::Manual => unreachable!("handled above"),
            };
            Some((key, worker.id.clone()))
        })
        .max_by_key(|(key, _)| *key)
        .map(|(_, id)| id)
}

/// Choose a ready provider subscription from advertised budget headroom.
///
/// Manual routing returns `None`, preserving an explicit task hint or the
/// daemon's configured default. Automatic strategies also return `None` when no
/// provider reports remaining tokens, keeping budget metadata advisory and
/// fail-open rather than blocking a valid host default.
pub(super) fn subscription_for_strategy(
    capabilities: &crate::protocol::AgentCapabilities,
    strategy: crate::runtime::SubscriptionRoutingStrategy,
) -> Option<crate::protocol::HarnessProvider> {
    use crate::runtime::SubscriptionRoutingStrategy;

    if strategy == SubscriptionRoutingStrategy::Manual {
        return None;
    }
    capabilities
        .providers
        .iter()
        .enumerate()
        .filter(|(_, provider)| {
            capabilities
                .readiness
                .iter()
                .find(|readiness| readiness.provider == **provider)
                .map(|readiness| readiness.ready)
                .unwrap_or(true)
        })
        .filter_map(|(index, provider)| {
            let budget = capabilities
                .budgets
                .iter()
                .find(|budget| budget.provider == *provider)?;
            let remaining = budget.remaining_tokens?.max(0);
            let ratio = match budget.limit_tokens {
                Some(limit) if limit > 0 => i128::from(remaining) * 1_000_000 / i128::from(limit),
                _ => 0,
            };
            let preference = usize::MAX - index;
            let key = match strategy {
                SubscriptionRoutingStrategy::Balanced => (ratio, i128::from(remaining), preference),
                SubscriptionRoutingStrategy::MostAvailableBudget => {
                    (i128::from(remaining), ratio, preference)
                }
                SubscriptionRoutingStrategy::Manual => unreachable!("handled above"),
            };
            Some((key, *provider))
        })
        .max_by_key(|(key, _)| *key)
        .map(|(_, provider)| provider)
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

mod types;
pub use types::SharedRoster;
pub use types::{HubWorker, SharedSubscriptionStrategy};
