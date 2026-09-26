//! Data types for the Subconscious signal graph: the nodes, the edges between
//! them, and the simulation state that animates the whole thing.

/// A node's role in the workflow, which decides its color, size, and glyph.
///
/// The kind is the only thing the renderer needs to style a node, so the graph
/// builder can stay a flat table of tuples rather than carrying styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NodeKind {
    /// Where work enters the orchestrator from: a human, a webhook, a schedule.
    Source,
    /// Medulla itself, the hub every source feeds and every agent hangs off.
    Orchestrator,
    /// A first-class agent the orchestrator spawned directly.
    Agent,
    /// A subagent or tool an agent spawned in turn.
    Subagent,
    /// A node that feeds work back upstream — the head of a review loop.
    Loop,
    /// A terminal node: where the run's output lands.
    Sink,
}

impl NodeKind {
    /// The node's base color as linear RGB, before pulse and depth fading.
    pub(crate) fn rgb(self) -> (f64, f64, f64) {
        match self {
            NodeKind::Source => (95.0, 200.0, 255.0),
            NodeKind::Orchestrator => (198.0, 132.0, 255.0),
            NodeKind::Agent => (120.0, 226.0, 168.0),
            NodeKind::Subagent => (255.0, 176.0, 96.0),
            NodeKind::Loop => (255.0, 104.0, 152.0),
            NodeKind::Sink => (140.0, 168.0, 255.0),
        }
    }

    /// The marker drawn immediately left of the node's label.
    pub(crate) fn glyph(self) -> char {
        match self {
            NodeKind::Source => '◆',
            NodeKind::Orchestrator => '◉',
            NodeKind::Agent => '●',
            NodeKind::Subagent => '•',
            NodeKind::Loop => '↺',
            NodeKind::Sink => '▣',
        }
    }

    /// How hard the anchor spring holds this node in place. The orchestrator is
    /// held almost rigidly so the fan never drifts off its centre.
    pub(crate) fn anchor_stiffness(self) -> f64 {
        match self {
            NodeKind::Orchestrator => 0.30,
            NodeKind::Source => 0.055,
            _ => 0.035,
        }
    }
}

/// What an edge means, which decides its color and the direction work flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeKind {
    /// A source feeding the orchestrator.
    Feed,
    /// The orchestrator (or an agent) spawning something downstream.
    Spawn,
    /// A result travelling back upstream to be reworked.
    Feedback,
}

impl EdgeKind {
    /// The edge's color as linear RGB.
    pub(crate) fn rgb(self) -> (f64, f64, f64) {
        match self {
            EdgeKind::Feed => (70.0, 150.0, 210.0),
            EdgeKind::Spawn => (120.0, 110.0, 190.0),
            EdgeKind::Feedback => (200.0, 80.0, 120.0),
        }
    }
}

/// One node of the graph: its identity, where it wants to be, and where the
/// simulation has actually put it.
#[derive(Debug, Clone)]
pub(crate) struct Node {
    /// Short display name, drawn beside the node's glyph.
    pub(crate) label: &'static str,
    /// The node's role, which drives all of its styling.
    pub(crate) kind: NodeKind,
    /// Anchor position in normalised graph space, `[-1, 1]` on both axes. The
    /// simulation springs the node back toward this, so the fan keeps its shape
    /// however the forces push.
    pub(crate) anchor: (f64, f64),
    /// Current simulated position, in the same space as `anchor`.
    pub(crate) pos: (f64, f64),
    /// Current velocity, in graph units per step.
    pub(crate) vel: (f64, f64),
    /// Per-node offset into the wiggle waveform, so no two nodes breathe in
    /// lockstep. Derived from the node's index at build time.
    pub(crate) phase: f64,
    /// Which side of its own glyph the label is drawn on. Set at build time
    /// because it depends on where the node sits in the fan, and flipping it
    /// per-frame as a node drifts would make labels jitter.
    pub(crate) label_left: bool,
}

/// A directed connection between two nodes, by index into [`Graph::nodes`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Edge {
    /// Index of the node work flows from.
    pub(crate) from: usize,
    /// Index of the node work flows to.
    pub(crate) to: usize,
    /// What the connection means.
    pub(crate) kind: EdgeKind,
}

/// The whole animated graph: topology plus the frame counter that drives it.
#[derive(Debug, Clone)]
pub(crate) struct Graph {
    /// Every node, in build order. Index 0 is the orchestrator by convention.
    pub(crate) nodes: Vec<Node>,
    /// Every edge, referring to `nodes` by index.
    pub(crate) edges: Vec<Edge>,
    /// Steps advanced so far. Drives the wiggle, the pulses, and the packets;
    /// it is the only clock the graph has, which keeps rendering deterministic
    /// and therefore testable.
    pub(crate) frame: u64,
}

impl Graph {
    /// Elapsed animation time in arbitrary units, derived from [`Self::frame`].
    ///
    /// One step is one render tick (~90ms), so this advances at roughly 1.0 per
    /// second at the app's normal draw cadence.
    pub(crate) fn time(&self) -> f64 {
        self.frame as f64 * 0.09
    }
}

impl Default for Graph {
    fn default() -> Self {
        super::mock::mock_graph()
    }
}
