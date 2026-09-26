//! The 2D force simulation that animates the workflow graph.
//!
//! This is a damped spring system rather than a free force-directed layout: the
//! composition comes from the hand-placed anchors in [`super::mock`], and the
//! forces here only ever perturb it. That is a deliberate trade — a free layout
//! re-solves its own shape every run and lands somewhere different (and often
//! unreadable) each time, whereas the panel needs to look the same every time an
//! operator opens the tab while still feeling alive.
//!
//! Every force is deterministic — the "wiggle" is a sum of sines over the frame
//! counter, not a random walk — so a given frame number always renders the same
//! picture and the tests can assert on it.

use super::types::Graph;

/// How strongly nodes push each other apart, in graph units.
const REPULSION: f64 = 0.000_25;
/// Beyond this distance nodes stop repelling, which keeps the far side of the
/// fan from being pushed off canvas by the crowded side.
const REPULSION_RANGE: f64 = 0.34;
/// Spring constant pulling connected nodes toward `EDGE_REST` apart.
const EDGE_K: f64 = 0.010;
/// The separation an edge is happy at, in graph units.
const EDGE_REST: f64 = 0.30;
/// Velocity retained per step. Low enough that the system never oscillates.
const DAMPING: f64 = 0.86;
/// Amplitude of the idle wiggle, in graph units.
const WIGGLE: f64 = 0.0016;
/// Hard cap on speed, so a transient force spike cannot fling a node.
const MAX_SPEED: f64 = 0.02;
/// Nodes are clamped inside this box to stay on canvas.
const BOUND: f64 = 1.0;

/// Advance the simulation by one frame.
///
/// Called once per render tick. Doing physics from the draw path is unusual, but
/// this graph has no other clock and no state outside itself: the frame counter
/// it bumps is the only thing that changes.
pub(crate) fn step(graph: &mut Graph) {
    graph.frame = graph.frame.wrapping_add(1);
    let t = graph.time();
    let n = graph.nodes.len();
    let mut force = vec![(0.0f64, 0.0f64); n];

    // Pairwise repulsion, so clusters of subagents spread out instead of
    // overprinting each other's labels.
    for i in 0..n {
        for j in (i + 1)..n {
            let (dx, dy) = (
                graph.nodes[i].pos.0 - graph.nodes[j].pos.0,
                graph.nodes[i].pos.1 - graph.nodes[j].pos.1,
            );
            let dist_sq = dx * dx + dy * dy;
            if dist_sq > REPULSION_RANGE * REPULSION_RANGE {
                continue;
            }
            // Floor the distance so coincident nodes get a strong but finite
            // shove rather than a division by zero.
            let dist = dist_sq.sqrt().max(0.02);
            let mag = REPULSION / (dist * dist);
            let (ux, uy) = (dx / dist, dy / dist);
            force[i].0 += ux * mag;
            force[i].1 += uy * mag;
            force[j].0 -= ux * mag;
            force[j].1 -= uy * mag;
        }
    }

    // Edge springs: connected work stays visibly connected.
    for edge in &graph.edges {
        let (a, b) = (edge.from, edge.to);
        let (dx, dy) = (
            graph.nodes[b].pos.0 - graph.nodes[a].pos.0,
            graph.nodes[b].pos.1 - graph.nodes[a].pos.1,
        );
        let dist = (dx * dx + dy * dy).sqrt().max(0.02);
        let pull = (dist - EDGE_REST) * EDGE_K;
        let (ux, uy) = (dx / dist, dy / dist);
        force[a].0 += ux * pull;
        force[a].1 += uy * pull;
        force[b].0 -= ux * pull;
        force[b].1 -= uy * pull;
    }

    for (i, node) in graph.nodes.iter_mut().enumerate() {
        // Anchor spring — the force that actually preserves the composition.
        let stiffness = node.kind.anchor_stiffness();
        force[i].0 += (node.anchor.0 - node.pos.0) * stiffness;
        force[i].1 += (node.anchor.1 - node.pos.1) * stiffness;

        // Idle wiggle. Two incommensurate frequencies per axis so the motion
        // never settles into a visible loop.
        let p = node.phase;
        force[i].0 += WIGGLE * ((t * 0.9 + p).sin() + 0.5 * (t * 1.7 + p * 1.3).sin());
        force[i].1 += WIGGLE * ((t * 1.1 + p * 0.7).cos() + 0.5 * (t * 2.3 + p).cos());

        node.vel.0 = (node.vel.0 + force[i].0) * DAMPING;
        node.vel.1 = (node.vel.1 + force[i].1) * DAMPING;
        let speed = (node.vel.0 * node.vel.0 + node.vel.1 * node.vel.1).sqrt();
        if speed > MAX_SPEED {
            node.vel.0 *= MAX_SPEED / speed;
            node.vel.1 *= MAX_SPEED / speed;
        }
        node.pos.0 = (node.pos.0 + node.vel.0).clamp(-BOUND, BOUND);
        node.pos.1 = (node.pos.1 + node.vel.1).clamp(-BOUND, BOUND);
    }
}
