//! Laying a workflow graph out for a terminal, and moving a cursor through it.
//!
//! A [`WorkflowGraph`] is a set of nodes and edges with no geometry the engine
//! cares about — the optional `position` on a node is a canvas hint written by a
//! visual editor, and most graphs (an agent's, a hand-written file's) carry none
//! at all. A terminal cannot use pixel coordinates anyway: it has a character
//! grid, and what a reader needs from it is *order* — what runs first, what
//! branches, what waits for what.
//!
//! So the layout here is derived rather than read: nodes are layered by how far
//! they are from a trigger (a longest-path ranking, so a node never sits left of
//! anything that feeds it), and ordered inside a layer to follow their parents.
//! The result is a column per step of the plan and a lane per concurrent branch,
//! which is the shape the run itself has.
//!
//! Geometry only — no ratatui, no styling. The app crate turns a
//! [`GraphLayout`] into boxes and box-drawing characters, and [`Move`] steps the
//! cursor over the same structure. Both are testable without a terminal.

use std::collections::{BTreeMap, HashMap, HashSet};

use tinyflows::model::{Node, NodeKind, WorkflowGraph};

/// Which way a cursor is being moved through the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// Downstream: toward what this node feeds.
    Forward,
    /// Upstream: toward what feeds this node.
    Back,
    /// The previous lane in the same layer.
    LaneUp,
    /// The next lane in the same layer.
    LaneDown,
}

/// One node, placed on the character grid's *cell* lattice.
///
/// `layer` and `lane` are cell coordinates, not columns and rows: the renderer
/// decides how wide a node box is and how much air goes between layers, and it
/// can change that (a narrow terminal, a zoomed-out view) without the layout
/// being recomputed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedNode {
    /// The node's id in the graph.
    pub id: String,
    /// Display name, falling back to the id when the document leaves it empty.
    pub name: String,
    /// The node kind on the JSON wire (`agent`, `http_request`, …).
    pub kind: String,
    /// A one-character marker for the kind, so a box says what it is before its
    /// label is read.
    pub glyph: &'static str,
    /// Distance from a root, in layers. Column order, left to right.
    pub layer: usize,
    /// Position within the layer. Lane order, top to bottom.
    pub lane: usize,
    /// The single most useful line of the node's config — the model an agent
    /// runs on, the URL a request hits — or empty when the kind has none.
    pub summary: String,
    /// Named output ports, when the kind declares more than the default one.
    pub out_ports: Vec<String>,
}

/// One edge, in terms of the nodes it joins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedEdge {
    /// Source node id.
    pub from: String,
    /// Target node id.
    pub to: String,
    /// The source port, when it is not the default `main` — this is what tells a
    /// reader which side of a condition they are following.
    pub label: Option<String>,
    /// The source node's layer.
    pub from_layer: usize,
    /// The source node's lane.
    pub from_lane: usize,
    /// The target node's layer.
    pub to_layer: usize,
    /// The target node's lane.
    pub to_lane: usize,
}

impl PlacedEdge {
    /// Whether this edge runs backwards — a loop closing on an earlier layer.
    ///
    /// Worth knowing separately because it cannot be drawn as a rightward arrow
    /// and reads as a mistake if it is.
    pub fn is_back_edge(&self) -> bool {
        self.to_layer <= self.from_layer
    }
}

/// A whole graph, laid out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphLayout {
    /// Every node, in layer-then-lane order — which is also a sensible reading
    /// order and the order a keyboard walks with Tab.
    pub nodes: Vec<PlacedNode>,
    /// Every edge.
    pub edges: Vec<PlacedEdge>,
    /// How many layers the graph has.
    pub layers: usize,
    /// The widest layer's lane count.
    pub lanes: usize,
}

impl GraphLayout {
    /// Lay `graph` out.
    ///
    /// Nodes with no incoming edge start at layer 0; every other node sits one
    /// layer past the furthest thing that feeds it. A cycle would make that
    /// definition circular, so the relaxation is bounded by the node count and
    /// whatever it has settled on by then is used — a graph with a loop still
    /// draws, which matters because the operator is often looking at it to work
    /// out *why* it has one.
    pub fn build(graph: &WorkflowGraph) -> Self {
        let ids: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
        let known: HashSet<&str> = ids.iter().map(String::as_str).collect();
        // Edges naming a node that is not in the graph are dropped rather than
        // panicking on lookup: a half-edited document is a normal thing to be
        // looking at, and validation reports the dangling reference already.
        let edges: Vec<&tinyflows::model::Edge> = graph
            .edges
            .iter()
            .filter(|edge| known.contains(edge.from_node.as_str()))
            .filter(|edge| known.contains(edge.to_node.as_str()))
            .collect();

        // A loop's closing edge is not a statement about depth, so it is taken
        // out before anything is ranked. Left in, the relaxation below chases it
        // around the cycle once per pass and the graph comes out as many layers
        // deep as it has nodes — a ten-step workflow with one loop laid out
        // sixty-three columns wide, folded into twenty-one bands of mostly
        // empty canvas.
        let back = back_edges(&ids, &edges);
        let forward: Vec<&tinyflows::model::Edge> = edges
            .iter()
            .enumerate()
            .filter(|(index, _)| !back.contains(index))
            .map(|(_, edge)| *edge)
            .collect();

        let layer = layer_nodes(&ids, &forward);
        let lane = lane_nodes(&ids, &forward, &layer);

        let mut nodes: Vec<PlacedNode> = graph
            .nodes
            .iter()
            .map(|node| PlacedNode {
                id: node.id.clone(),
                name: display_name(node),
                kind: kind_wire(&node.kind).to_string(),
                glyph: kind_glyph(&node.kind),
                layer: layer.get(&node.id).copied().unwrap_or(0),
                lane: lane.get(&node.id).copied().unwrap_or(0),
                summary: node_summary(node),
                out_ports: out_ports(node),
            })
            .collect();
        nodes.sort_by_key(|node| (node.layer, node.lane));

        let placed_edges = edges
            .iter()
            .map(|edge| PlacedEdge {
                from: edge.from_node.clone(),
                to: edge.to_node.clone(),
                label: (edge.from_port != "main").then(|| edge.from_port.clone()),
                from_layer: layer.get(&edge.from_node).copied().unwrap_or(0),
                from_lane: lane.get(&edge.from_node).copied().unwrap_or(0),
                to_layer: layer.get(&edge.to_node).copied().unwrap_or(0),
                to_lane: lane.get(&edge.to_node).copied().unwrap_or(0),
            })
            .collect();

        let layers = nodes.iter().map(|node| node.layer + 1).max().unwrap_or(0);
        let lanes = nodes.iter().map(|node| node.lane + 1).max().unwrap_or(0);

        Self {
            nodes,
            edges: placed_edges,
            layers,
            lanes,
        }
    }

    /// The node at `index` in reading order.
    pub fn node(&self, index: usize) -> Option<&PlacedNode> {
        self.nodes.get(index)
    }

    /// The reading-order index of the node with `id`.
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.nodes.iter().position(|node| node.id == id)
    }

    /// Every node in one layer, in lane order.
    pub fn layer_nodes(&self, layer: usize) -> Vec<&PlacedNode> {
        self.nodes
            .iter()
            .filter(|node| node.layer == layer)
            .collect()
    }

    /// Move the cursor from `index`, returning where it lands.
    ///
    /// Forward and back follow *edges* rather than jumping to the next column,
    /// because following the wiring is what a reader is doing — and when a node
    /// has no edge that way, the nearest node in the adjacent layer is used so
    /// the cursor is never stuck on a disconnected fragment.
    ///
    /// Returns `None` when there is nowhere to go, which the caller renders as
    /// "the cursor stays put" rather than an error.
    pub fn moved(&self, index: usize, direction: Move) -> Option<usize> {
        let current = self.nodes.get(index)?;
        match direction {
            Move::LaneUp | Move::LaneDown => {
                let up = direction == Move::LaneUp;
                self.nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, node)| node.layer == current.layer)
                    .filter(|(_, node)| {
                        if up {
                            node.lane < current.lane
                        } else {
                            node.lane > current.lane
                        }
                    })
                    // The *nearest* lane in that direction, so a cursor steps
                    // through lanes one at a time rather than jumping the layer.
                    .min_by_key(|(_, node)| node.lane.abs_diff(current.lane))
                    .map(|(index, _)| index)
            }
            // A loop's closing edge is a successor that sits *behind* its
            // source, so following edge order blindly would send `Forward` back
            // up the graph and `Back` down it — the cursor appearing to move
            // the wrong way. Prefer an edge that actually travels the requested
            // direction, and fall back to the layer scan otherwise; the loop is
            // still reachable, by pressing the direction it genuinely lies in.
            Move::Forward => self
                .successors(&current.id)
                .into_iter()
                .filter_map(|id| self.index_of(id))
                .find(|index| self.nodes[*index].layer > current.layer)
                .or_else(|| self.nearest_in_layer(current, current.layer + 1)),
            Move::Back => self
                .predecessors(&current.id)
                .into_iter()
                .filter_map(|id| self.index_of(id))
                .find(|index| self.nodes[*index].layer < current.layer)
                .or_else(|| {
                    current
                        .layer
                        .checked_sub(1)
                        .and_then(|layer| self.nearest_in_layer(current, layer))
                }),
        }
    }

    /// The nodes `id` feeds, in the graph's own edge order.
    pub fn successors(&self, id: &str) -> Vec<&str> {
        self.edges
            .iter()
            .filter(|edge| edge.from == id)
            .map(|edge| edge.to.as_str())
            .collect()
    }

    /// The nodes that feed `id`, in the graph's own edge order.
    pub fn predecessors(&self, id: &str) -> Vec<&str> {
        self.edges
            .iter()
            .filter(|edge| edge.to == id)
            .map(|edge| edge.from.as_str())
            .collect()
    }

    /// The node in `layer` whose lane is closest to `from`'s.
    fn nearest_in_layer(&self, from: &PlacedNode, layer: usize) -> Option<usize> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.layer == layer)
            .min_by_key(|(_, node)| node.lane.abs_diff(from.lane))
            .map(|(index, _)| index)
    }
}

/// The edges that close a cycle, by index into `edges`.
///
/// A depth-first walk marks an edge as a back edge when its target is already on
/// the walk's own stack — the standard definition, and the one that agrees with
/// how a reader sees a loop: the arm that returns to a step already taken.
///
/// Roots are walked first so the walk follows the plan's own direction and the
/// arm it calls "back" is the arm an author would. A component that is all cycle
/// has no root, so every node is tried afterwards as well; there the choice of
/// which edge closes the loop is arbitrary, but exactly one is removed either
/// way and the rest of the component still ranks.
fn back_edges(ids: &[String], edges: &[&tinyflows::model::Edge]) -> HashSet<usize> {
    let mut out: HashMap<&str, Vec<(usize, &str)>> = HashMap::new();
    for (index, edge) in edges.iter().enumerate() {
        out.entry(edge.from_node.as_str())
            .or_default()
            .push((index, edge.to_node.as_str()));
    }
    let targets: HashSet<&str> = edges.iter().map(|edge| edge.to_node.as_str()).collect();
    let roots = ids.iter().filter(|id| !targets.contains(id.as_str()));

    let mut back: HashSet<usize> = HashSet::new();
    let mut visited: HashSet<&str> = HashSet::new();
    // The nodes on the current path, which is what distinguishes a back edge
    // from an edge that merely rejoins a branch explored earlier.
    let mut on_path: HashSet<&str> = HashSet::new();
    // Iterative rather than recursive: the cursor is how far through a node's
    // outgoing edges the walk has got.
    let mut stack: Vec<(&str, usize)> = Vec::new();

    for start in roots.chain(ids.iter()) {
        let start = start.as_str();
        if !visited.insert(start) {
            continue;
        }
        on_path.insert(start);
        stack.push((start, 0));
        while let Some(&(node, cursor)) = stack.last() {
            let children = out.get(node).map(Vec::as_slice).unwrap_or(&[]);
            let Some(&(index, child)) = children.get(cursor) else {
                on_path.remove(node);
                stack.pop();
                continue;
            };
            if let Some(top) = stack.last_mut() {
                top.1 += 1;
            }
            if on_path.contains(child) {
                back.insert(index);
            } else if visited.insert(child) {
                on_path.insert(child);
                stack.push((child, 0));
            }
        }
    }
    back
}

/// Assign each node a layer: one past the furthest node that feeds it.
///
/// Relaxed rather than sorted topologically, because it is cheap and because the
/// caller has already taken the cycles out — see [`back_edges`]. `ids.len()`
/// passes settle any acyclic graph; the bound stays as a backstop in case an
/// edge that should have been removed was not.
fn layer_nodes(ids: &[String], edges: &[&tinyflows::model::Edge]) -> HashMap<String, usize> {
    let mut layer: HashMap<String, usize> = ids.iter().map(|id| (id.clone(), 0)).collect();
    for _ in 0..ids.len() {
        let mut changed = false;
        for edge in edges {
            // A self-edge cannot push a node right of itself; skipping it keeps
            // the relaxation from running away to the pass limit.
            if edge.from_node == edge.to_node {
                continue;
            }
            let want = layer[&edge.from_node] + 1;
            if layer[&edge.to_node] < want {
                layer.insert(edge.to_node.clone(), want);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    layer
}

/// Assign each node a lane within its layer.
///
/// Ordered by the mean lane of whatever feeds it, so a branch stays on the line
/// it branched from and edges cross as little as the layering allows. Ties — and
/// every node in layer 0, which has no parents — keep the document's own order,
/// which is the order an author wrote and therefore the order they expect.
fn lane_nodes(
    ids: &[String],
    edges: &[&tinyflows::model::Edge],
    layer: &HashMap<String, usize>,
) -> HashMap<String, usize> {
    let position: HashMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect();
    let mut by_layer: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
    for id in ids {
        by_layer
            .entry(layer.get(id).copied().unwrap_or(0))
            .or_default()
            .push(id.as_str());
    }

    let mut lane: HashMap<String, usize> = HashMap::new();
    for (_, mut members) in by_layer {
        // Sorted on the parents' *already assigned* lanes, which is why this
        // walks layers in ascending order: layer n's order depends on n-1's.
        let key = |id: &str| -> (usize, usize) {
            let parents: Vec<usize> = edges
                .iter()
                .filter(|edge| edge.to_node == id)
                .filter_map(|edge| lane.get(&edge.from_node).copied())
                .collect();
            let mean = if parents.is_empty() {
                usize::MAX
            } else {
                parents.iter().sum::<usize>() * 10 / parents.len()
            };
            (mean, position.get(id).copied().unwrap_or(0))
        };
        members.sort_by_key(|id| key(id));
        for (index, id) in members.into_iter().enumerate() {
            lane.insert(id.to_string(), index);
        }
    }
    lane
}

/// The name to show for a node: its own, or its id when it has none.
fn display_name(node: &Node) -> String {
    let name = node.name.trim();
    if name.is_empty() {
        node.id.clone()
    } else {
        name.to_string()
    }
}

/// A node kind as it appears on the JSON wire.
pub fn kind_wire(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Trigger => "trigger",
        NodeKind::Agent => "agent",
        NodeKind::ToolCall => "tool_call",
        NodeKind::HttpRequest => "http_request",
        NodeKind::Code => "code",
        NodeKind::Shell => "shell",
        NodeKind::Condition => "condition",
        NodeKind::Switch => "switch",
        NodeKind::Merge => "merge",
        NodeKind::SplitOut => "split_out",
        NodeKind::Transform => "transform",
        NodeKind::OutputParser => "output_parser",
        NodeKind::SubWorkflow => "sub_workflow",
        NodeKind::Memory => "memory",
        NodeKind::Dedup => "dedup",
        NodeKind::Loop => "loop",
        NodeKind::Spawn => "spawn",
        NodeKind::Gate => "gate",
        NodeKind::Scatter => "scatter",
        NodeKind::Gather => "gather",
    }
}

/// A one-character marker for a node kind.
///
/// Chosen so the *shape* carries the meaning: an entry point, a branch, a
/// barrier, a fan-out. On a terminal that draws them at all this is read before
/// the label, and on one that does not the label is still there.
pub fn kind_glyph(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Trigger => "▶",
        NodeKind::Agent => "✦",
        NodeKind::ToolCall => "⚒",
        NodeKind::HttpRequest => "⇅",
        NodeKind::Code => "λ",
        // A prompt, because that is what the step is: a script handed to a
        // shell, not a computation the graph carries.
        NodeKind::Shell => "$",
        NodeKind::Condition => "◆",
        NodeKind::Switch => "⑂",
        NodeKind::Merge => "⊕",
        NodeKind::SplitOut => "⋔",
        NodeKind::Transform => "ƒ",
        NodeKind::OutputParser => "⌗",
        NodeKind::SubWorkflow => "⧉",
        // A store read or written, not a step that transforms what passes
        // through it — the shape says "shelf" rather than "arrow".
        NodeKind::Memory => "▤",
        // A filter that lets each key past once: what it drops is a repeat, so
        // the shape says "not equal to what already went through".
        NodeKind::Dedup => "≠",
        // The one kind whose edges run backwards: the shape is the cycle it
        // draws on the canvas.
        NodeKind::Loop => "↺",
        // Work that leaves the branch and comes back later: the arrow points
        // away from the path the graph is still walking.
        NodeKind::Spawn => "⇗",
        // The collecting half, pointing back in. Read beside a `spawn` the pair
        // reads as one round trip.
        NodeKind::Gate => "⇘",
        // Fan-out and fan-in of the whole downstream path. Deliberately the
        // heavier pair of arrows: unlike `split_out`, what widens here is the
        // pipeline, not the item stream.
        NodeKind::Scatter => "⇶",
        NodeKind::Gather => "⇉",
    }
}

/// A colour name for a node kind, in the vocabulary the app crate maps to a
/// theme.
///
/// Grouped by what a node *does* rather than one colour per kind: the entry
/// point, the things that reach outside the process, the pure transforms, and
/// the control flow. Twelve colours would be noise; four are a legend.
pub fn kind_color(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Trigger => "green",
        NodeKind::Agent | NodeKind::SubWorkflow => "magenta",
        // Grouped with the reaching-outside kinds: a memory node's result comes
        // from the host's store, not from anything the graph carries.
        NodeKind::ToolCall
        | NodeKind::HttpRequest
        | NodeKind::Code
        | NodeKind::Shell
        | NodeKind::Memory => "cyan",
        // `dedup` reads durable state, but what it *does* to the graph is route:
        // an item either continues or is dropped, so it reads with the control
        // flow rather than with the kinds that reach outside the process.
        // `spawn`/`gate` and `scatter`/`gather` read with the control flow for
        // the same reason: what each does to the graph is decide where and how
        // wide execution goes next. That the work itself may reach outside the
        // process is the business of the nodes they start, not of these.
        NodeKind::Condition
        | NodeKind::Switch
        | NodeKind::Merge
        | NodeKind::SplitOut
        | NodeKind::Dedup
        | NodeKind::Loop
        | NodeKind::Spawn
        | NodeKind::Gate
        | NodeKind::Scatter
        | NodeKind::Gather => "yellow",
        NodeKind::Transform | NodeKind::OutputParser => "blue",
    }
}

/// The colour for a node kind named by its wire string.
///
/// The layout carries a node's kind as a string rather than the engine enum, so
/// a renderer holding a [`PlacedNode`] has the name and not the variant. An
/// unrecognised kind — one a newer document declares and this build has never
/// heard of — is drawn in the default colour rather than not drawn at all.
pub fn color_for_kind(wire: &str) -> &'static str {
    serde_json::from_value::<NodeKind>(serde_json::Value::String(wire.to_string()))
        .map(|kind| kind_color(&kind))
        .unwrap_or("white")
}

/// The one line of a node's config worth showing beside its name.
///
/// Per kind, because what identifies a node differs: an HTTP request is its URL,
/// an agent is the instruction it carries, a condition is the expression it
/// tests. Returns empty when the config says nothing useful, which the renderer
/// treats as "no second line" rather than printing a blank.
pub fn node_summary(node: &Node) -> String {
    let text = |key: &str| -> Option<String> {
        node.config
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.split('\n').next().unwrap_or(value).to_string())
    };
    match node.kind {
        NodeKind::Trigger => text("trigger_kind").unwrap_or_default(),
        NodeKind::Agent => text("prompt")
            .or_else(|| text("instruction"))
            .or_else(|| text("agent_ref"))
            .unwrap_or_default(),
        NodeKind::ToolCall => text("tool").or_else(|| text("action")).unwrap_or_default(),
        NodeKind::HttpRequest => {
            let method = text("method").unwrap_or_else(|| "GET".to_string());
            match text("url") {
                Some(url) => format!("{} {url}", method.to_uppercase()),
                None => String::new(),
            }
        }
        NodeKind::Code => text("language").unwrap_or_default(),
        // The script itself, when it is inline and short enough to read; the
        // path otherwise, which is the other thing that says what will run.
        NodeKind::Shell => text("script")
            .or_else(|| text("script_path"))
            .unwrap_or_default(),
        NodeKind::Condition => text("expression")
            .or_else(|| text("left"))
            .unwrap_or_default(),
        NodeKind::Switch => text("expression").unwrap_or_default(),
        NodeKind::SplitOut => text("path").unwrap_or_default(),
        NodeKind::Transform => text("expression").unwrap_or_default(),
        NodeKind::OutputParser => text("schema_ref").unwrap_or_default(),
        NodeKind::SubWorkflow => text("workflow_id").unwrap_or_default(),
        // The operation is what the node does; the scope is what it does it to,
        // and reading `remember` without knowing where is the ambiguous half.
        NodeKind::Memory => match (text("operation"), text("scope")) {
            (Some(operation), Some(scope)) => format!("{operation} · {scope}"),
            (Some(operation), None) => operation,
            (None, _) => String::new(),
        },
        // The key expression is the whole of what a dedup node decides by.
        NodeKind::Dedup => text("key").unwrap_or_default(),
        // The cap is the number an operator scans for; the condition, when
        // there is one, is why the loop might stop before reaching it.
        NodeKind::Loop => {
            let max = node
                .config
                .get("max_iterations")
                .and_then(|value| value.as_u64())
                .map_or_else(|| "default".to_string(), |n| n.to_string());
            match text("condition") {
                Some(condition) => format!("max {max} · while {condition}"),
                None => format!("max {max}"),
            }
        }
        NodeKind::Merge => node
            .config
            .get("inputs")
            .and_then(|value| value.as_array())
            .map(|inputs| format!("{} inputs", inputs.len()))
            .unwrap_or_default(),
    }
}

/// The node's declared output ports, minus the implicit `main`.
///
/// The interesting ones are the named branches — `true`/`false` on a condition,
/// a switch's cases — because those are what an edge label refers to.
fn out_ports(node: &Node) -> Vec<String> {
    node.ports
        .iter()
        .map(|port| port.name.clone())
        .filter(|name| name != "main")
        .collect()
}

#[cfg(test)]
#[path = "graph_tests.rs"]
mod tests;
