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

    if !capacity.templates.is_empty() {
        rows.push(section("agent templates"));
        for template in &capacity.templates {
            rows.push(template_row(template));
        }
    }

    rows
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

/// One template row: what may be provisioned, and where it is allowed to run.
fn template_row(template: &crate::runtime::fleet::AgentTemplate) -> FleetNode {
    let harnesses: Vec<String> = template.harnesses.keys().cloned().collect();
    FleetNode {
        key: format!("template:{}", template.id),
        kind: FleetNodeKind::Template,
        depth: 1,
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
            template.tags.join(", "),
        ]),
        degraded: false,
    }
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
