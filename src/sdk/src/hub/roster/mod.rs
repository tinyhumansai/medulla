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

/// The `AgentDescriptor` a single worker advertises.
///
/// Only ever built for a worker that survived the liveness filter, so
/// `availability` is always `"online"` here. It stays on the wire because the
/// orchestrator only auto-assigns an untargeted task to an agent whose
/// availability is exactly `"online"`, and a blank one would be silently
/// excluded from every fan-out.
fn to_agent(w: &HubWorker) -> Value {
    // `metadata.workspace` is what places the agent: the backend turns it into a
    // WorkspaceDescriptor and sets the agent's `workspaceId` from it. Omitted
    // rather than sent empty when unknown, so the backend falls through to the
    // worker's probed `capabilities.cwd` instead of placing it at "".
    let mut metadata = json!({ "address": w.address, "harness": w.harness });
    // How many sessions this agent may run at once, derived from its declared
    // strategy. Code-plane data: deterministic placement reads it to decide
    // whether an agent has headroom, and no prompt ever does (spec §4.2).
    //
    // A zero is withheld rather than sent. Capacity of nothing reads as
    // "saturated" — the opposite of the permissive default every other omission
    // here means — and an agent that never stated a strategy should be treated
    // as available, not as full.
    if w.max_sessions > 0 {
        metadata["maxSessions"] = json!(w.max_sessions);
    }
    // The path, not the `{path, type}` object the entity model carries. The
    // backend parses both, so widening it buys nothing today and would only
    // churn every reader of this advert; the type rides along with the
    // remote-host work that gives a worktree a reason to be named.
    if let Some(workspace) = w.workspace_path() {
        metadata["workspace"] = json!(workspace);
    }
    // Control state is deliberately NOT advertised — not the hold, not its
    // reason, not since when, and not the handback brief that only exists
    // because of one.
    //
    // It used to be, and it was right when a worker *was* an agent *was* a
    // machine *was* one implicit session: "this worker is held" and "this
    // session is held" were the same sentence. They are not any more. An agent
    // now runs N sessions ([`HubWorker::max_sessions`]), a person takes *one* of
    // them, and this flag has no room to say which — so a backend that folds
    // held-state onto its ledger by `agentId` would mark every pending task on
    // the agent as held, including the ones running perfectly well in other
    // sessions. Emitting something wrong is worse than emitting nothing: the
    // wrong thing is acted on.
    //
    // Saying it correctly needs session identity on the wire, which is only
    // actionable once a dispatch can name a session (spec §C3, deferred). Until
    // then this stays local: [`HubWorker::control`] and the whole take /
    // hand-back path are unchanged and still decide medulla's *own* dispatch
    // (`session_for` skips a session an operator holds; a held in-flight task
    // suspends and is delivered by the hand-back turn). None of it crosses the
    // wire, and the backend learns what happened the only way that cannot be
    // mis-keyed: through the task's own result.
    //
    // The host this agent runs on, when this hub knows which one. The backend
    // prefers a supplied id and only synthesizes `host:${socketId}` as a last
    // resort, so saying it here is what stops five machines behind one hub
    // socket from collapsing into one synthetic host.
    //
    // Blank means this hub did not say — a remote peer the operator added by
    // address, which has no declared host — and is omitted rather than sent
    // empty so the backend's synthesis still applies to exactly those.
    let mut agent = json!({
        "id": w.id,
        // The name falls back to the id, not to a second constant. `agent_list`
        // renders `id (name)`, so two different readable tokens put the wrong
        // answer back on the table — which is the whole failure being fixed
        // here. Unlabelled, the two coincide and there is nothing to get wrong;
        // labelled, the id is a visible slug of the name.
        "name": w.label.clone().unwrap_or_else(|| w.id.clone()),
        // What this worker is: the harness that runs the work. The role
        // catalog that used to refine this went with the orchestration engine.
        "description": format!("{} daemon", w.harness),
        "availability": "online",
        // Every one of these runs a coding harness, so `code` is what it is:
        // dropping the tag would take the worker out of the fan-outs that ask
        // for code.
        "tags": ["code"],
        "metadata": metadata,
    });
    // `hostId` goes ONLY on an agent with no workspace. The library's contract
    // (`AgentDescriptor.hostId`) is explicit: it names the host a *local* agent
    // runs on, "only meaningful when `workspaceId` is absent", and must NEVER be
    // set on a harness-backed agent, whose host is derived by walking up from
    // its workspace. Setting it on every agent made the server skip synthesizing
    // a `workspaceId` from `metadata.workspace` (`a supplied workspaceId or
    // hostId always wins`), which orphaned every agent from the
    // agent→workspace→harness→host chain: `host_list` still rendered them, but
    // placement reported "no agent inside <host> is available (none declared
    // there)" and no task could be dispatched. The `hosts[]` block carries host
    // identity for the topology; a workspace-backed agent must not repeat it.
    if w.workspace_path().is_none() {
        if let Some(host_id) = host_id_of(w) {
            agent["hostId"] = json!(host_id);
        }
    }
    agent
}

/// The host id a worker is placed on, when it declares one.
///
/// Trimmed, and blank reads as "not declared" rather than as a host whose id is
/// the empty string — which would key every unplaced worker to the same
/// non-existent host.
fn host_id_of(w: &HubWorker) -> Option<&str> {
    let host_id = w.host_id.trim();
    (!host_id.is_empty()).then_some(host_id)
}

/// The `hosts[]` block: one entry per host the advertised agents are placed on.
///
/// Derived from the agents rather than listed from config alone, so the two
/// halves of one payload cannot disagree: every `hostId` an agent carries has an
/// entry here, and no entry describes a host with nothing on it. A host whose
/// agents were all withheld by the liveness filter is therefore withheld too —
/// the same rule the agent list follows, applied one level up.
///
/// `declared` is what this machine declares locally
/// ([`local_hosts`](crate::config::local_hosts)), and is the whole of how `kind`
/// is decided: a host this machine declares is `local`, and any other host an
/// agent names is one the hub merely fronts, so it is `remote`. Nothing is
/// probed to establish this, per the declaration doctrine (spec §2.1).
///
/// `resources` is deliberately never emitted. The hub holds per-*worker*
/// capability probes, not host-level facts, and aggregating those into a host
/// resource claim would be inventing a number — the backend drops what it
/// cannot validate anyway, so an absent block is honest where a synthesised one
/// would not be.
fn to_hosts(advertised: &[&HubWorker], declared: &[crate::config::LocalHostRef]) -> Vec<Value> {
    let mut hosts: Vec<Value> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for w in advertised {
        let Some(host_id) = host_id_of(w) else {
            continue;
        };
        if seen.contains(&host_id) {
            continue;
        }
        seen.push(host_id);
        let local = declared.iter().find(|host| host.id == host_id);
        let mut entry = json!({
            "hostId": host_id,
            "kind": if local.is_some() { "local" } else { "remote" },
        });
        // The name only exists for a host this machine declared; a host learned
        // from an agent's placement has an id and nothing else to call it.
        // Omitted rather than defaulted to the id, which the backend can do
        // itself and which would otherwise look like an operator's choice.
        if let Some(name) = local
            .map(|host| host.name.trim())
            .filter(|name| !name.is_empty())
        {
            entry["name"] = json!(name);
        }
        // The address its agents are reached at — the same value they advertise
        // as `metadata.address`, taken from the agent rather than re-derived so
        // the two can never disagree.
        let address = w.address.trim();
        if !address.is_empty() {
            entry["address"] = json!(address);
        }
        hosts.push(entry);
    }
    hosts
}

/// The roster entry a [`WorkerSpec`](crate::hub::WorkerSpec) describes.
///
/// The one place a spec becomes a live roster row, so the declaration → advert
/// chain has a single seam: an agent's workspace and derived capacity arrive
/// here from what the operator declared, and `to_agent` turns them into
/// `metadata`.
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
    declared_hosts: &[crate::config::LocalHostRef],
) -> Value {
    let reachable: Vec<&HubWorker> = workers.iter().filter(|w| is_reachable(w, online)).collect();
    let hosts = to_hosts(&reachable, declared_hosts);
    let mut payload = json!({
        "agents": reachable
            .iter()
            .map(|w| to_agent(w))
            .collect::<Vec<_>>()
    });
    // Omitted rather than sent empty. `hosts: []` is a key that says nothing —
    // no agent named a host — and this payload is re-emitted on every roster
    // mutation, so a key that carries no information is only noise in a diff.
    if !hosts.is_empty() {
        payload["hosts"] = json!(hosts);
    }
    payload
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

/// Resolve a targeted `agentId` to a link address.
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
pub use types::{HubWorker, SharedLocalHosts};
