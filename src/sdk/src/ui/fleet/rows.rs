//! Flatten the declared capacity and the roster into the Fleet list's rows.
//!
//! The walk is top-down over the containment chain — host, then its harnesses,
//! then their workspaces, then the agents deployed into them — followed by the
//! agents that hang off a host directly and, last, the template catalog.
//!
//! Two rules keep a half-synced fleet legible rather than lossy. Nothing is
//! dropped for a dangling parent: a harness whose host is not declared still
//! renders, under an "unknown host" group named for the id it claims. And
//! nothing is invented: an agent with no resolvable placement lands in a
//! trailing `unplaced` group rather than being adopted by an arbitrary host.

use std::collections::BTreeSet;

use crate::runtime::fleet::CapacitySnapshot;
use crate::runtime::AgentDescriptor;

use super::fmt;
use super::types::{FleetNode, FleetNodeKind};

/// Build the flattened fleet tree for `capacity`, placing `roster` agents in it.
///
/// Returns an empty vector when nothing is declared and no agent exists, which
/// callers render as the view's empty state rather than as a bare tree.
pub fn fleet_rows(capacity: &CapacitySnapshot, roster: &[AgentDescriptor]) -> Vec<FleetNode> {
    let mut rows = Vec::new();
    let mut placed: BTreeSet<&str> = BTreeSet::new();

    for host in &capacity.hosts {
        rows.push(host_row(host, capacity, roster));
        push_harnesses(&mut rows, &host.id, capacity, roster, &mut placed);
        // Agents pinned straight to this host: local agents, and anything the
        // backend reported without a workspace to walk up from.
        for agent in roster
            .iter()
            .filter(|a| a.workspace_id.is_none() && a.host_id.as_deref() == Some(host.id.as_str()))
        {
            placed.insert(agent.id.as_str());
            rows.push(agent_row(agent, capacity, 1));
        }
    }

    // Harnesses whose declared host is missing: still real capacity, grouped
    // under the id they name so the dangling link is visible instead of silent.
    let orphan_hosts: Vec<&str> = capacity
        .harnesses
        .iter()
        .filter(|h| capacity.host(&h.host_id).is_none())
        .map(|h| h.host_id.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for host_id in orphan_hosts {
        rows.push(FleetNode {
            key: format!("host:{host_id}"),
            kind: FleetNodeKind::Host,
            depth: 0,
            label: host_id.to_string(),
            detail: "undeclared host".into(),
            degraded: true,
        });
        push_harnesses(&mut rows, host_id, capacity, roster, &mut placed);
    }

    let unplaced: Vec<&AgentDescriptor> = roster
        .iter()
        .filter(|a| !placed.contains(a.id.as_str()))
        .collect();
    if !unplaced.is_empty() {
        rows.push(section("unplaced agents"));
        for agent in unplaced {
            rows.push(agent_row(agent, capacity, 1));
        }
    }

    rows
}

/// Build the agent-template catalog's rows: one per declared template, each
/// annotated with where it may run and how much of the roster came from it.
///
/// A separate list from [`fleet_rows`] on purpose. A template is not a level of
/// the containment chain — it is a constraint *across* it, saying what may be
/// stood up rather than where — so hanging it off the tree implied a parent it
/// does not have.
pub fn template_rows(capacity: &CapacitySnapshot, roster: &[AgentDescriptor]) -> Vec<FleetNode> {
    capacity
        .templates
        .iter()
        .map(|template| template_row(template, capacity, roster))
        .collect()
}

/// Append one host's harnesses, their workspaces, and the agents in them.
fn push_harnesses<'a>(
    rows: &mut Vec<FleetNode>,
    host_id: &str,
    capacity: &'a CapacitySnapshot,
    roster: &'a [AgentDescriptor],
    placed: &mut BTreeSet<&'a str>,
) {
    for harness in capacity.harnesses.iter().filter(|h| h.host_id == host_id) {
        let budget = fmt::tightest(&harness.budgets)
            .map(fmt::budget)
            .unwrap_or_default();
        rows.push(FleetNode {
            key: format!("harness:{}", harness.id),
            kind: FleetNodeKind::Harness,
            depth: 1,
            label: harness.kind.clone(),
            detail: fmt::join(&[
                if harness.ready {
                    "ready".into()
                } else {
                    harness
                        .ready_reason
                        .clone()
                        .map(|r| format!("not ready: {r}"))
                        .unwrap_or_else(|| "not ready".into())
                },
                budget,
                harness.availability.clone(),
            ]),
            degraded: !harness.ready || fmt::is_offline(&harness.availability),
        });

        for workspace in capacity
            .workspaces
            .iter()
            .filter(|w| w.harness_id == harness.id)
        {
            rows.push(FleetNode {
                key: format!("workspace:{}", workspace.id),
                kind: FleetNodeKind::Workspace,
                depth: 2,
                label: workspace.path.clone(),
                detail: fmt::join(&[
                    workspace.project.clone().unwrap_or_default(),
                    workspace
                        .metadata
                        .get("branch")
                        .and_then(|b| b.as_str())
                        .unwrap_or_default()
                        .to_string(),
                ]),
                degraded: false,
            });
            for agent in roster
                .iter()
                .filter(|a| a.workspace_id.as_deref() == Some(workspace.id.as_str()))
            {
                placed.insert(agent.id.as_str());
                rows.push(agent_row(agent, capacity, 3));
            }
        }
    }
}

/// One host row, annotated with how much of the roster sits under it.
fn host_row(
    host: &crate::runtime::fleet::HostDescriptor,
    capacity: &CapacitySnapshot,
    roster: &[AgentDescriptor],
) -> FleetNode {
    let agents = roster
        .iter()
        .filter(|a| capacity.placement(a).host.map(|h| h.id.as_str()) == Some(host.id.as_str()))
        .count();
    FleetNode {
        key: format!("host:{}", host.id),
        kind: FleetNodeKind::Host,
        depth: 0,
        label: if host.name.trim().is_empty() {
            host.id.clone()
        } else {
            host.name.clone()
        },
        detail: fmt::join(&[
            host.availability.clone(),
            fmt::resources(host.resources.as_ref()),
            match agents {
                0 => String::new(),
                1 => "1 agent".into(),
                n => format!("{n} agents"),
            },
        ]),
        degraded: fmt::is_offline(&host.availability),
    }
}

/// One agent row at the given depth, tagged with its provisioning template.
fn agent_row(agent: &AgentDescriptor, capacity: &CapacitySnapshot, depth: usize) -> FleetNode {
    let template = capacity
        .template(agent.template_id.as_deref().unwrap_or_default())
        .map(|t| format!("via {}", t.name.clone().unwrap_or_else(|| t.id.clone())))
        .or_else(|| agent.template_id.clone().map(|id| format!("via {id}")))
        .unwrap_or_default();
    FleetNode {
        key: format!("agent:{}", agent.id),
        kind: FleetNodeKind::Agent,
        depth,
        label: if agent.name.trim().is_empty() {
            agent.id.clone()
        } else {
            agent.name.clone()
        },
        detail: fmt::join(&[agent.availability.clone(), template, agent.tags.join(", ")]),
        degraded: fmt::is_offline(&agent.availability),
    }
}

/// One template row: what may be provisioned, where it is allowed to run, and
/// how many agents currently exist because of it.
fn template_row(
    template: &crate::runtime::fleet::AgentTemplate,
    capacity: &CapacitySnapshot,
    roster: &[AgentDescriptor],
) -> FleetNode {
    let harnesses: Vec<String> = template.harnesses.keys().cloned().collect();
    let provisioned = roster
        .iter()
        .filter(|a| a.template_id.as_deref() == Some(template.id.as_str()))
        .count();
    FleetNode {
        key: format!("template:{}", template.id),
        kind: FleetNodeKind::Template,
        depth: 0,
        label: template.name.clone().unwrap_or_else(|| template.id.clone()),
        detail: fmt::join(&[
            template.model.clone().unwrap_or_default(),
            // A `harnesses` block is a restriction, not just an override set:
            // its keys enumerate the only kinds this template may run on.
            if harnesses.is_empty() {
                String::new()
            } else {
                format!("on {}", harnesses.join(", "))
            },
            match places_allowing(template, capacity) {
                0 => "nowhere allows it".into(),
                1 => "1 place".into(),
                n => format!("{n} places"),
            },
            match provisioned {
                0 => String::new(),
                1 => "1 agent".into(),
                n => format!("{n} agents"),
            },
            template.tags.join(", "),
        ]),
        // A template no declared place admits cannot be provisioned anywhere,
        // which is worth seeing at a glance rather than only on selection.
        degraded: places_allowing(template, capacity) == 0,
    }
}

/// How many declared workspaces admit this template.
///
/// A `templateIds` allowlist only ever subtracts: absent means inherit, so a
/// workspace that declares none inherits its harness's list, and a harness that
/// declares none admits the whole catalog. Enforcement is fail-open at the
/// config level for exactly this reason.
pub fn places_allowing(
    template: &crate::runtime::fleet::AgentTemplate,
    capacity: &CapacitySnapshot,
) -> usize {
    capacity
        .workspaces
        .iter()
        .filter(|workspace| {
            let harness = capacity.harness(&workspace.harness_id);
            // A `harnesses` block on the template restricts it to those kinds.
            if !template.harnesses.is_empty()
                && !harness
                    .map(|h| template.harnesses.contains_key(&h.kind))
                    .unwrap_or(false)
            {
                return false;
            }
            let allowed_here =
                workspace.template_ids.is_empty() || workspace.template_ids.contains(&template.id);
            let allowed_on_harness = harness
                .map(|h| h.template_ids.is_empty() || h.template_ids.contains(&template.id))
                .unwrap_or(true);
            allowed_here && allowed_on_harness
        })
        .count()
}

/// A non-selectable heading row.
fn section(label: &str) -> FleetNode {
    FleetNode {
        key: format!("section:{label}"),
        kind: FleetNodeKind::Section,
        depth: 0,
        label: format!("── {label} ──"),
        detail: String::new(),
        degraded: false,
    }
}
