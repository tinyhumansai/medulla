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
    details: &HashMap<String, crate::tinyplace::WorkerSystemInfo>,
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
    capabilities: &crate::tinyplace::AgentCapabilities,
    strategy: crate::runtime::SubscriptionRoutingStrategy,
) -> Option<crate::tinyplace::HarnessProvider> {
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
