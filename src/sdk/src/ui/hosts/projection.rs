//! Folding the declarations and the live roster into the `Host → Agents` tree.
//!
//! The one place the two sources meet. Which of them is authoritative depends on
//! the host: a local host's agents come from what this machine wrote down, a
//! remote host's from what the roster reached (see [`super`]) — so the join is
//! stated once, here, rather than being re-derived by each tab that draws it.

use super::{HostAgentRow, HostKind, HostRow};
use crate::config::LocalHostRef;
use crate::runtime::{AgentDeclaration, WorkerInfo};

/// Build the `Host → Agents` tree.
///
/// `locals` is every host this machine declares (see
/// [`local_hosts`](crate::config::local_hosts)); it leads the result in that
/// order, whether or not anything is running on it — a declared host with
/// nothing in the roster is still where its agents live, and hiding it would
/// make "the local host is always present" false exactly when the operator needs
/// to see why nothing is being dispatched.
///
/// `workers` is the live roster. Entries whose address matches a local host fill
/// in what is running there; the rest are grouped by address into one remote
/// host each, in first-seen order.
///
/// A declaration naming a host that `locals` does not contain still gets a host
/// row of its own, after the configured ones: it was declared on this machine,
/// so it is local and editable, and dropping it would hide agents the operator
/// wrote down.
pub fn host_rows(
    workers: &[WorkerInfo],
    declarations: &[AgentDeclaration],
    locals: &[LocalHostRef],
) -> Vec<HostRow> {
    let mut rows: Vec<HostRow> = Vec::new();
    let mut claimed: Vec<&str> = Vec::new();

    // The first local host also claims the agents that name no host at all: an
    // agent nothing places belongs to the machine looking at it. They are
    // declared in *this* config, so the alternative is not "somewhere else" but
    // "nowhere" — an agent written down and rendered by neither tab.
    for (index, local) in locals.iter().enumerate() {
        rows.push(local_row(
            &local.id,
            &local.name,
            workers,
            declarations,
            index == 0,
            &mut claimed,
        ));
    }
    // Declared, but on a host id this config does not (or no longer) describes.
    // An unplaced agent lands here only when there is no local host to claim it,
    // which is the one arrangement where it would otherwise vanish.
    for declaration in declarations {
        let host_id = declaration.host_id.trim();
        if rows.iter().any(|row| row.id == host_id) {
            continue;
        }
        if host_id.is_empty() && !locals.is_empty() {
            continue;
        }
        let label = if host_id.is_empty() {
            "this device"
        } else {
            host_id
        };
        rows.push(local_row(
            host_id,
            label,
            workers,
            declarations,
            host_id.is_empty(),
            &mut claimed,
        ));
    }
    // Everything left in the roster is somebody else's machine.
    for worker in workers {
        if claimed.contains(&worker.id.as_str()) {
            continue;
        }
        let address = worker.address.trim();
        match rows
            .iter_mut()
            .find(|row| row.kind == HostKind::Remote && row.id == address)
        {
            Some(row) => {
                // The host preview reads its capacity, readiness and budgets off
                // this one entry, so it has to be an entry that has any: keeping
                // whichever agent arrived first left the machine reading "not
                // reported" while a probed sibling on the same address held the
                // answer. A probed pick is never given up for a later one.
                let probed = row
                    .detail_worker
                    .as_deref()
                    .and_then(|id| workers.iter().find(|held| held.id == id))
                    .is_some_and(has_probe_details);
                if row.detail_worker.is_none() || (!probed && has_probe_details(worker)) {
                    row.detail_worker = Some(worker.id.clone());
                }
                // One agent per id: the same peer can reach the registry twice
                // (added by hand, then advertised), and an agent listed twice is
                // an agent an operator thinks they have two of.
                if !row.agents.iter().any(|agent| agent.agent_id == worker.id) {
                    row.agents.push(agent_from_worker(worker, false));
                }
            }
            None => rows.push(HostRow {
                id: address.to_string(),
                label: remote_label(worker),
                kind: HostKind::Remote,
                agents: vec![agent_from_worker(worker, false)],
                detail_worker: Some(worker.id.clone()),
            }),
        }
    }
    rows
}

/// One local host: its declared agents first, then anything the roster has at
/// its address that no declaration claims.
///
/// The undeclared tail is the migration seed (an install that predates
/// declarations advertises agents nobody wrote down) and it stays editable:
/// assigning it a role is what writes the declaration that makes the role stick.
///
/// `unplaced` makes this row the home for declarations that name no host — see
/// [`host_rows`]. Exactly one row ever sets it, or an agent would be listed
/// under every local host at once.
fn local_row<'a>(
    id: &str,
    label: &str,
    workers: &'a [WorkerInfo],
    declarations: &[AgentDeclaration],
    unplaced: bool,
    claimed: &mut Vec<&'a str>,
) -> HostRow {
    let mut agents: Vec<HostAgentRow> = Vec::new();
    for declaration in declarations
        .iter()
        .filter(|d| d.on_host(id) || (unplaced && d.host_id.trim().is_empty()))
    {
        let worker = workers
            .iter()
            .find(|worker| worker.id.trim() == declaration.agent_id.trim());
        agents.push(agent_from_declaration(declaration, worker));
    }
    let mut detail_worker = None;
    for worker in workers.iter().filter(|worker| worker.address.trim() == id) {
        if detail_worker.is_none() && has_probe_details(worker) {
            detail_worker = Some(worker.id.clone());
        }
        if agents.iter().any(|agent| agent.agent_id == worker.id) {
            continue;
        }
        agents.push(agent_from_worker(worker, true));
    }
    // Claim by agent id rather than by address: a declaration may name an agent
    // whose roster entry is registered under another address, and it must not
    // then be listed a second time as a remote host of its own.
    for agent in &agents {
        if let Some(worker) = workers.iter().find(|worker| worker.id == agent.agent_id) {
            claimed.push(worker.id.as_str());
        }
    }
    if detail_worker.is_none() {
        detail_worker = agents
            .iter()
            .find(|agent| agent.live)
            .map(|agent| agent.agent_id.clone());
    }
    HostRow {
        id: id.to_string(),
        label: label.to_string(),
        kind: HostKind::Local,
        agents,
        detail_worker,
    }
}

/// Whether a roster entry carries a capability probe worth previewing.
fn has_probe_details(worker: &WorkerInfo) -> bool {
    worker.cpu_cores.is_some()
        || worker.memory_total_bytes.is_some()
        || worker.memory_available_bytes.is_some()
        || worker.ip_address.is_some()
        || !worker.readiness.is_empty()
        || !worker.budgets.is_empty()
}

/// An agent row from the declaration that defines it, with whatever the roster
/// knows about it folded in.
///
/// Roles come from the declaration, not from the live entry: the declaration is
/// what survives a restart, so showing it is what makes the checkbox honest.
fn agent_from_declaration(
    declaration: &AgentDeclaration,
    worker: Option<&WorkerInfo>,
) -> HostAgentRow {
    let workspace = declaration
        .workspace
        .path()
        .map(str::to_string)
        .or_else(|| worker.and_then(|worker| worker.workspace.clone()));
    HostAgentRow {
        agent_id: declaration.agent_id.clone(),
        // A blank name is not a name: a declaration whose name field holds only
        // spaces would otherwise render as a row with nothing on it.
        label: declaration
            .name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| worker.and_then(|worker| worker.label.clone()))
            .unwrap_or_else(|| declaration.agent_id.clone()),
        harness: Some(declaration.harness.clone()),
        workspace,
        roles: declaration.roles.clone(),
        max_sessions: Some(declaration.max_sessions()),
        declared: true,
        editable: true,
        live: worker.is_some(),
        selected: worker.is_some_and(|worker| worker.selected),
    }
}

/// An agent row for a roster entry no declaration on this machine covers.
///
/// `local` says whether it sits on a host this machine runs, which is the only
/// thing that decides whether its roles can be assigned here.
fn agent_from_worker(worker: &WorkerInfo, local: bool) -> HostAgentRow {
    HostAgentRow {
        agent_id: worker.id.clone(),
        label: worker
            .label
            .clone()
            .or_else(|| worker.handle.clone())
            .unwrap_or_else(|| worker.id.clone()),
        harness: worker.harness.clone(),
        workspace: worker.workspace.clone(),
        roles: worker.roles.clone(),
        max_sessions: None,
        declared: false,
        editable: local,
        live: true,
        selected: worker.selected,
    }
}

/// What to call a remote host: the operator's label, its handle, else the raw
/// address they typed.
fn remote_label(worker: &WorkerInfo) -> String {
    worker
        .label
        .clone()
        .or_else(|| worker.handle.clone())
        .unwrap_or_else(|| worker.address.clone())
}
