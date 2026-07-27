//! What changed between two versions of a graph.
//!
//! The copilot edits the workflow store directly — it calls `workflow_apply_ops`
//! and the file on disk is different afterwards. That is the right design (there
//! is one authoring path and every surface shares it) but it leaves the operator
//! reading a reply that says "done" with no account of what "done" was.
//!
//! So the turn snapshots the graph before and after and reports the difference.
//! It is derived rather than trusted: the agent's own summary of its edit is
//! what it *meant* to do, and this is what the store actually holds.

use tinyflows::model::{Node, WorkflowGraph};

use crate::ui::workflows::graph::kind_wire;

/// One change between two graphs, already phrased for a transcript line.
pub type Change = String;

/// Describe how `after` differs from `before`, newest structure first.
///
/// Nodes are matched by id, so a renamed id reads as a removal plus an addition.
/// That is the honest reading — the engine treats it as one too, since every
/// edge names the id — and pretending otherwise would hide the edges that had to
/// be rewired along with it.
///
/// Returns empty when nothing changed, which the caller reports as "no edit"
/// rather than saying nothing: a turn that claimed to change the graph and did
/// not is exactly what an operator needs to be told.
pub fn describe(before: &WorkflowGraph, after: &WorkflowGraph) -> Vec<Change> {
    let mut changes = Vec::new();

    for node in &after.nodes {
        match find(before, &node.id) {
            None => changes.push(format!("+ node {} ({})", node.id, kind_wire(&node.kind))),
            Some(old) => changes.extend(node_changes(old, node)),
        }
    }
    for node in &before.nodes {
        if find(after, &node.id).is_none() {
            changes.push(format!("− node {} ({})", node.id, kind_wire(&node.kind)));
        }
    }

    for edge in &after.edges {
        if !before.edges.contains(edge) {
            changes.push(format!("+ edge {}", edge_label(edge)));
        }
    }
    for edge in &before.edges {
        if !after.edges.contains(edge) {
            changes.push(format!("− edge {}", edge_label(edge)));
        }
    }

    if before.name != after.name {
        changes.push(format!("~ name {} → {}", before.name, after.name));
    }

    changes
}

/// The per-field changes to a node that exists in both graphs.
///
/// The config is compared whole rather than key by key: a node's config is a
/// kind-specific document, and "the agent config changed" plus the node's name
/// is what a transcript line has room for. The inspector beside it shows the
/// new value in full.
fn node_changes(before: &Node, after: &Node) -> Vec<Change> {
    let mut changes = Vec::new();
    if before.kind != after.kind {
        changes.push(format!(
            "~ node {} kind {} → {}",
            after.id,
            kind_wire(&before.kind),
            kind_wire(&after.kind)
        ));
    }
    if before.name != after.name {
        changes.push(format!(
            "~ node {} named \"{}\"",
            after.id,
            after.name.trim()
        ));
    }
    if before.config != after.config {
        changes.push(format!("~ node {} config", after.id));
    }
    if before.ports != after.ports {
        changes.push(format!("~ node {} ports", after.id));
    }
    changes
}

/// `from → to`, naming the ports only when they are not the default.
fn edge_label(edge: &tinyflows::model::Edge) -> String {
    let from = if edge.from_port == "main" {
        edge.from_node.clone()
    } else {
        format!("{}:{}", edge.from_node, edge.from_port)
    };
    let to = if edge.to_port == "main" {
        edge.to_node.clone()
    } else {
        format!("{}:{}", edge.to_node, edge.to_port)
    };
    format!("{from} → {to}")
}

/// The node with `id`, if the graph has one.
fn find<'a>(graph: &'a WorkflowGraph, id: &str) -> Option<&'a Node> {
    graph.nodes.iter().find(|node| node.id == id)
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
