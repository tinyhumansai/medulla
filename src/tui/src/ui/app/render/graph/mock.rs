//! The demonstration signal graph shown on the Subconscious tab.
//!
//! This is deliberately mock topology, not live run state: it shows the *shape*
//! a Medulla run takes — many input sources funnelling into one orchestrator,
//! which fans out into a deep tree of agents and subagents with review loops
//! folding back on themselves. Wiring it to real runtime data is a later change;
//! the node and edge types it builds are already the general ones.
//!
//! It is sized at the busy end of what a real run looks like on purpose. A
//! sparse demo graph would hide exactly the problem the panel has to survive —
//! dozens of nodes competing for the same few hundred cells.

use super::types::{Edge, EdgeKind, Graph, Node, NodeKind};

/// Index of the orchestrator, which every source feeds and every agent hangs
/// off. Fixed at 0 so callers can reference the hub without searching.
const HUB: usize = 0;

/// Build the mock fan-out graph.
///
/// Anchors are hand-placed in normalised `[-1, 1]` space rather than derived
/// from a tree layout: the fan needs to read well at a fixed, fairly small
/// terminal size, and hand placement is what actually achieves that. The
/// simulation only ever perturbs these positions, so the composition survives.
///
/// The columns run left to right — sources, hub, agents, then three generations
/// of spawned work — because that is the direction the run reads in.
pub(crate) fn mock_graph() -> Graph {
    // (label, kind, anchor x, anchor y)
    let spec: &[(&'static str, NodeKind, f64, f64)] = &[
        ("medulla", NodeKind::Orchestrator, -0.45, 0.00),
        // Inputs, fanning in from the left.
        ("manual", NodeKind::Source, -0.80, 0.70),
        ("github", NodeKind::Source, -0.92, 0.42),
        ("slack", NodeKind::Source, -0.97, 0.13),
        ("schedule", NodeKind::Source, -0.97, -0.17),
        ("webhook", NodeKind::Source, -0.92, -0.45),
        ("api", NodeKind::Source, -0.80, -0.72),
        // First-class agents.
        ("planner", NodeKind::Agent, -0.12, 0.62),
        ("coder", NodeKind::Agent, -0.06, 0.20),
        ("research", NodeKind::Agent, -0.10, -0.26),
        ("ops", NodeKind::Agent, -0.16, -0.68),
        // What those agents spawned in turn.
        ("spec", NodeKind::Subagent, 0.20, 0.87),
        ("critic", NodeKind::Loop, 0.26, 0.62),
        ("impl·a", NodeKind::Subagent, 0.22, 0.35),
        ("impl·b", NodeKind::Subagent, 0.28, 0.12),
        ("web", NodeKind::Subagent, 0.20, -0.14),
        ("docs", NodeKind::Subagent, 0.24, -0.38),
        ("deploy", NodeKind::Subagent, 0.18, -0.62),
        ("alerts", NodeKind::Subagent, 0.26, -0.87),
        // A third generation: subagents of subagents.
        ("outline", NodeKind::Subagent, 0.54, 0.90),
        ("review", NodeKind::Loop, 0.60, 0.66),
        ("tests", NodeKind::Subagent, 0.56, 0.43),
        ("lint", NodeKind::Subagent, 0.62, 0.22),
        ("fetch", NodeKind::Subagent, 0.54, -0.04),
        ("parse", NodeKind::Subagent, 0.60, -0.25),
        ("index", NodeKind::Subagent, 0.54, -0.45),
        ("canary", NodeKind::Subagent, 0.60, -0.67),
        ("paging", NodeKind::Subagent, 0.56, -0.90),
        // The deep tail, and where the run lands.
        ("merge", NodeKind::Sink, 0.88, 0.86),
        ("bench", NodeKind::Subagent, 0.86, 0.54),
        ("fuzz", NodeKind::Subagent, 0.90, 0.31),
        ("e2e", NodeKind::Loop, 0.88, 0.08),
        ("summary", NodeKind::Subagent, 0.86, -0.16),
        ("rerank", NodeKind::Subagent, 0.90, -0.39),
        ("rollback", NodeKind::Loop, 0.86, -0.62),
        ("ship", NodeKind::Sink, 0.88, -0.88),
    ];

    let nodes = spec
        .iter()
        .enumerate()
        .map(|(i, &(label, kind, x, y))| Node {
            label,
            kind,
            anchor: (x, y),
            pos: (x, y),
            vel: (0.0, 0.0),
            // Irrational stride so the phases spread evenly however many nodes
            // there are, instead of landing in a few visible groups.
            phase: i as f64 * 2.399_963,
            // Sources sit against the left edge and the deep tail against the
            // right, so both need their labels turned inward to stay on canvas.
            label_left: kind == NodeKind::Source || x > 0.7,
        })
        .collect::<Vec<_>>();

    let by_label = |label: &str| {
        spec.iter()
            .position(|entry| entry.0 == label)
            .expect("mock graph edges only name nodes the table defines")
    };
    let mut edges = Vec::new();
    let mut connect = |from: &str, to: &str, kind: EdgeKind| {
        edges.push(Edge {
            from: by_label(from),
            to: by_label(to),
            kind,
        });
    };
    for source in ["manual", "github", "slack", "schedule", "webhook", "api"] {
        connect(source, "medulla", EdgeKind::Feed);
    }
    for agent in ["planner", "coder", "research", "ops"] {
        connect("medulla", agent, EdgeKind::Spawn);
    }
    for (parent, child) in [
        ("planner", "spec"),
        ("planner", "critic"),
        ("coder", "impl·a"),
        ("coder", "impl·b"),
        ("research", "web"),
        ("research", "docs"),
        ("ops", "deploy"),
        ("ops", "alerts"),
        ("spec", "outline"),
        ("critic", "review"),
        ("impl·a", "tests"),
        ("impl·b", "lint"),
        ("web", "fetch"),
        ("web", "parse"),
        ("docs", "index"),
        ("deploy", "canary"),
        ("alerts", "paging"),
        ("outline", "merge"),
        ("tests", "bench"),
        ("tests", "fuzz"),
        ("lint", "e2e"),
        ("parse", "summary"),
        ("index", "rerank"),
        ("canary", "rollback"),
        ("canary", "ship"),
    ] {
        connect(parent, child, EdgeKind::Spawn);
    }
    // The loops: work that failed review goes back to whoever produced it.
    for (from, to) in [
        ("critic", "planner"),
        ("review", "spec"),
        ("e2e", "coder"),
        ("rollback", "ops"),
        ("summary", "research"),
    ] {
        connect(from, to, EdgeKind::Feedback);
    }

    debug_assert_eq!(nodes[HUB].kind, NodeKind::Orchestrator);
    Graph {
        nodes,
        edges,
        frame: 0,
    }
}
