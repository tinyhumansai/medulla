//! The graph as an outline: one step per row, branches indented under the gate
//! that chooses them.
//!
//! The wire canvas draws a graph the way it is shaped, which is right for a
//! genuinely parallel one and wrong for the common case. Most authored
//! workflows are a spine with a few gates hanging off it, and folding a spine
//! across a pane costs everything a reader wants: the names get clipped to a
//! column width, the chain wraps onto bands, and a two-arm branch is drawn as
//! wire routing rather than as a choice.
//!
//! Laid out as an outline the same graph is fifteen unclipped rows that scroll,
//! at any pane width, and the arms of a gate read as what they are. So the
//! outline is what a chain-shaped graph gets whenever the canvas would have had
//! to fold it — see [`App::prefers_outline`] — and `v` overrides the choice
//! either way.
//!
//! This module is the model: the row order, the duplicate-step grouping, and
//! the cursor's movement through both. The drawing is
//! [`crate::ui::app::render::workflows::outline`].

use std::collections::{HashMap, HashSet};

use medulla::ui::workflows::GraphLayout;

use super::super::types::App;

/// One row of the outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::ui::app) struct OutlineRow {
    /// The node this row draws, as an index into the layout's reading order.
    pub(in crate::ui::app) node: usize,
    /// How far the row is indented, in branch levels.
    pub(in crate::ui::app) depth: usize,
    /// The port that leads here, when this row is one arm of a branch.
    pub(in crate::ui::app) arm: Option<String>,
    /// Whether this arm is the last one drawn at its level, which is what
    /// decides between `├` and `╰`.
    pub(in crate::ui::app) last_arm: bool,
    /// Whether the walk reached this node a second time — a join, or a loop's
    /// target — so the row is a reference rather than a subtree.
    pub(in crate::ui::app) revisit: bool,
}

/// A set of terminal steps that run the same body.
///
/// Four "Final report (…)" steps whose script is the same script and whose
/// configs differ only in the reason string they pass it are one step written
/// four times, and a reader who does not know that spends four inspections
/// finding out. Every member carries the group, so the outline can draw them as
/// the one step they are and name what actually differs.
///
/// Grouped on the *body* — the script or prompt — rather than on the whole
/// declaration, because a group that required byte-identical configs would find
/// nothing: duplicated steps are duplicated precisely so each copy can be handed
/// a different argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::ui::app) struct SinkGroup {
    /// How many steps share this body.
    pub(in crate::ui::app) size: usize,
    /// What to call the shared step: the part of the members' names they agree
    /// on, or the first member's name when they agree on nothing.
    pub(in crate::ui::app) label: String,
    /// Every member's own name, in graph order.
    pub(in crate::ui::app) names: Vec<String>,
    /// The config paths the members disagree on — all that separates them.
    pub(in crate::ui::app) differs: Vec<String>,
}

impl App {
    /// Whether the outline is what this graph is drawn as.
    ///
    /// Automatic unless `v` has said otherwise: a chain-shaped graph that the
    /// canvas would have to fold is better read as an outline, and anything
    /// else — a genuine fan-out, or a chain that fits — keeps the wires.
    pub(in crate::ui::app) fn outline_view(&self) -> bool {
        self.wf.outline.unwrap_or_else(|| self.prefers_outline())
    }

    /// Whether this graph reads better as an outline than as folded wires.
    ///
    /// Two conditions, both necessary. The graph has to be *chain-shaped* —
    /// most of its nodes on one path — because an outline of a genuine fan-out
    /// is a list of siblings with the parallelism written out of it. And the
    /// canvas has to be *folding* it, because a graph that fits across the pane
    /// is already drawn as one readable chain and the wires say more.
    pub(in crate::ui::app) fn prefers_outline(&self) -> bool {
        let layout = self.workflow_layout();
        if layout.nodes.len() < 3 {
            return false;
        }
        // Before the first frame there is no pane to have folded anything, and
        // "it does not fit" is not a judgement that can be made from no width.
        // The wires are the answer until the graph has actually been drawn.
        if self.area.width == 0 {
            return false;
        }
        let folds = layout.layers > self.layers_per_band();
        folds && dominant_path(layout) * 5 >= layout.nodes.len() * 3
    }

    /// The outline's rows, in reading order.
    pub(in crate::ui::app) fn outline_rows(&self) -> Vec<OutlineRow> {
        rows_of(self.workflow_layout())
    }

    /// Which outline row holds the cursor's node.
    ///
    /// A join sits on two rows that share one `node_index` — the subtree it was
    /// first reached from, and the `revisit` row that references it — so the
    /// index alone cannot tell which is current. [`WorkflowsState::outline_row`]
    /// remembers the row outline navigation last landed on; it is trusted only
    /// while it still points at `node_index` (see its doc comment), and any
    /// other case — including a plain, non-revisit node, where both rows name
    /// the same line — falls back to resolving by node.
    pub(in crate::ui::app) fn outline_cursor_row(&self, rows: &[OutlineRow]) -> usize {
        if let Some(row) = rows.get(self.wf.outline_row) {
            if row.node == self.wf.node_index {
                return self.wf.outline_row;
            }
        }
        rows.iter()
            .position(|row| row.node == self.wf.node_index && !row.revisit)
            .or_else(|| rows.iter().position(|row| row.node == self.wf.node_index))
            .unwrap_or(0)
    }

    /// Move the cursor one row up or down the outline.
    ///
    /// The outline's own order, not the graph's lanes: in a list, down means the
    /// next line. A move off either end stays put, as everywhere else in the
    /// tab.
    pub(in crate::ui::app) fn move_outline_cursor(&mut self, down: bool) {
        let rows = self.outline_rows();
        if rows.is_empty() {
            return;
        }
        let current = self.outline_cursor_row(&rows);
        let next = if down {
            (current + 1).min(rows.len() - 1)
        } else {
            current.saturating_sub(1)
        };
        if next != current {
            self.wf.node_index = rows[next].node;
            self.wf.outline_row = next;
            self.wf.preview_scroll = 0;
            self.scroll_outline_to_cursor();
        }
    }

    /// Keep the cursor's row inside the outline's viewport.
    pub(in crate::ui::app) fn scroll_outline_to_cursor(&mut self) {
        let rows = self.outline_rows();
        let row = self.outline_cursor_row(&rows);
        let visible = self.visible_rows();
        if row < self.wf.canvas_row {
            self.wf.canvas_row = row;
        } else if row + 1 > self.wf.canvas_row + visible {
            self.wf.canvas_row = row + 1 - visible;
        }
    }

    /// The groups of duplicated terminal steps in the selected graph, by node id.
    pub(in crate::ui::app) fn duplicate_sinks(&self) -> HashMap<String, SinkGroup> {
        let mut groups: HashMap<String, SinkGroup> = HashMap::new();
        let Some(graph) = self.wf.graph.as_ref() else {
            return groups;
        };
        let layout = self.workflow_layout();
        let has_outgoing: HashSet<&str> =
            layout.edges.iter().map(|edge| edge.from.as_str()).collect();

        // Keyed by what the step actually runs, so two sinks group only when
        // running one runs the other's code. The name is deliberately not part
        // of the key: differing names is the whole thing being reported.
        let mut by_body: HashMap<String, Vec<(String, serde_json::Value)>> = HashMap::new();
        for node in &layout.nodes {
            if has_outgoing.contains(node.id.as_str()) {
                continue;
            }
            let Some(declared) = medulla::ui::workflows::find_node_in(graph, &node.id) else {
                continue;
            };
            let Some(body) = body_of(&declared.config) else {
                continue;
            };
            by_body
                .entry(format!("{}\u{1}{body}", node.kind))
                .or_default()
                .push((node.id.clone(), declared.config.clone()));
        }

        for members in by_body.into_values() {
            if members.len() < 2 {
                continue;
            }
            let names: Vec<String> = members
                .iter()
                .filter_map(|(id, _)| layout.index_of(id).and_then(|index| layout.node(index)))
                .map(|node| node.name.clone())
                .collect();
            let configs: Vec<&serde_json::Value> =
                members.iter().map(|(_, config)| config).collect();
            let group = SinkGroup {
                size: members.len(),
                label: shared_name(&names),
                names,
                differs: differing_paths(&configs),
            };
            for (id, _) in members {
                groups.insert(id, group.clone());
            }
        }
        groups
    }

    /// What the preview says about a step several siblings duplicate.
    pub(in crate::ui::app) fn duplicate_sink_note(&self, id: &str) -> Option<String> {
        let group = self.duplicate_sinks().get(id).cloned()?;
        let differs = if group.differs.is_empty() {
            "nothing".to_string()
        } else {
            group.differs.join(", ")
        };
        Some(format!(
            "≡ {} ×{} · the same step as {} · differs only in {differs}",
            group.label,
            group.size,
            group.names.join(" · "),
        ))
    }
}

/// The executable body of a node's config: what running the step actually runs.
///
/// `None` for a config with no body at all, which is a step there is no evidence
/// is duplicated rather than one duplicated with an empty body.
fn body_of(config: &serde_json::Value) -> Option<String> {
    let body = config
        .get("args")
        .and_then(|args| args.get("script"))
        .or_else(|| config.get("script"))
        .or_else(|| config.get("prompt"))
        .or_else(|| config.get("expression"))?;
    let body = body
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| body.to_string());
    (!body.trim().is_empty()).then_some(body)
}

/// The dotted config paths on which these declarations disagree.
///
/// This is the whole difference between two duplicated steps, so it is what the
/// reader is told: `differs only in args.env.WHY` says the duplication is a
/// parameter, and names the parameter.
fn differing_paths(configs: &[&serde_json::Value]) -> Vec<String> {
    let mut flattened: Vec<HashMap<String, String>> = Vec::new();
    for config in configs {
        let mut map = HashMap::new();
        flatten(config, String::new(), &mut map);
        flattened.push(map);
    }
    let mut paths: Vec<String> = flattened
        .iter()
        .flat_map(|map| map.keys().cloned())
        .collect::<HashSet<_>>()
        .into_iter()
        .filter(|path| {
            let first = flattened[0].get(path);
            flattened.iter().any(|map| map.get(path) != first)
        })
        .collect();
    paths.sort();
    paths
}

/// Flatten a JSON value into dotted path → rendered value.
fn flatten(value: &serde_json::Value, path: String, out: &mut HashMap<String, String>) {
    match value {
        serde_json::Value::Object(fields) => {
            for (key, field) in fields {
                let next = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                flatten(field, next, out);
            }
        }
        other => {
            out.insert(path, other.to_string());
        }
    }
}

/// How many nodes the longest path through the graph covers.
fn dominant_path(layout: &GraphLayout) -> usize {
    let mut longest = vec![0usize; layout.nodes.len()];
    // Layers are topological, so walking the nodes in reverse layer order means
    // every successor is already answered when its source is reached.
    let mut order: Vec<usize> = (0..layout.nodes.len()).collect();
    order.sort_by_key(|index| std::cmp::Reverse(layout.nodes[*index].layer));
    for index in order {
        let id = &layout.nodes[index].id;
        let best = layout
            .edges
            .iter()
            .filter(|edge| &edge.from == id && !edge.is_back_edge())
            .filter_map(|edge| layout.index_of(&edge.to))
            .filter(|target| layout.nodes[*target].layer > layout.nodes[index].layer)
            .map(|target| longest[target])
            .max()
            .unwrap_or(0);
        longest[index] = best + 1;
    }
    longest.into_iter().max().unwrap_or(0)
}

/// Lay the graph out as rows: the spine at depth zero, every other arm indented
/// under the node that chooses it.
fn rows_of(layout: &GraphLayout) -> Vec<OutlineRow> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    let mut roots: Vec<usize> = (0..layout.nodes.len())
        .filter(|index| {
            let id = &layout.nodes[*index].id;
            !layout
                .edges
                .iter()
                .any(|edge| &edge.to == id && !edge.is_back_edge())
        })
        .collect();
    // A cyclic graph can have no root at all; starting at the reading order's
    // first node is better than drawing nothing.
    if roots.is_empty() && !layout.nodes.is_empty() {
        roots.push(0);
    }
    for root in roots {
        walk(layout, root, 0, None, true, &mut seen, &mut rows);
    }
    // Anything the walk could not reach still belongs on screen — an orphaned
    // step is exactly the kind of mistake this view should make visible.
    for index in 0..layout.nodes.len() {
        if seen.insert(index) {
            rows.push(OutlineRow {
                node: index,
                depth: 0,
                arm: None,
                last_arm: true,
                revisit: false,
            });
        }
    }
    rows
}

/// Emit `index` and everything downstream of it.
fn walk(
    layout: &GraphLayout,
    index: usize,
    depth: usize,
    arm: Option<String>,
    last_arm: bool,
    seen: &mut HashSet<usize>,
    rows: &mut Vec<OutlineRow>,
) {
    let revisit = !seen.insert(index);
    rows.push(OutlineRow {
        node: index,
        depth,
        arm,
        last_arm,
        revisit,
    });
    // A join is drawn where it was first reached; reaching it again is a
    // reference to that row, and following it a second time would duplicate the
    // whole tail of the graph under one of the arms.
    if revisit {
        return;
    }

    let id = &layout.nodes[index].id;
    let mut outgoing: Vec<(usize, Option<String>)> = layout
        .edges
        .iter()
        .filter(|edge| &edge.from == id && !edge.is_back_edge())
        .filter_map(|edge| {
            layout
                .index_of(&edge.to)
                .map(|target| (target, edge.label.clone()))
        })
        .collect();
    if outgoing.is_empty() {
        return;
    }
    // The arm carrying the longest continuation is the spine, and it is drawn
    // last and unindented: an outline reads top to bottom, so the step after
    // this one should be the next line rather than something below a branch.
    let spine = outgoing
        .iter()
        .enumerate()
        .max_by_key(|(_, (target, _))| subtree_size(layout, *target))
        .map(|(position, _)| position)
        .unwrap_or(0);
    let spine_arm = outgoing.remove(spine);

    let count = outgoing.len();
    for (position, (target, label)) in outgoing.into_iter().enumerate() {
        walk(
            layout,
            target,
            depth + 1,
            label,
            position + 1 == count && spine_arm.1.is_none(),
            seen,
            rows,
        );
    }
    let (target, label) = spine_arm;
    // The spine keeps this node's depth when it is the plain continuation, and
    // is indented like any other arm when it is one choice among several — a
    // gate's `true` arm is not the same thing as the next step.
    let (depth, last) = match (&label, count) {
        (None, _) => (depth, true),
        (Some(_), _) => (depth + 1, true),
    };
    walk(layout, target, depth, label, last, seen, rows);
}

/// How many nodes hang off `index`, following forward edges only.
///
/// Used to pick a gate's spine arm. Counted rather than depth-measured so a wide
/// arm and a long one are both preferred to a single-step exit, which is what a
/// report leg usually is.
fn subtree_size(layout: &GraphLayout, index: usize) -> usize {
    let mut seen = HashSet::new();
    let mut stack = vec![index];
    while let Some(current) = stack.pop() {
        if !seen.insert(current) {
            continue;
        }
        let id = &layout.nodes[current].id;
        for edge in layout.edges.iter().filter(|edge| &edge.from == id) {
            if edge.is_back_edge() {
                continue;
            }
            if let Some(target) = layout.index_of(&edge.to) {
                stack.push(target);
            }
        }
    }
    seen.len()
}

/// What a set of near-identical names agree on, as a name in its own right.
///
/// `Final report (skip)` and `Final report (triage)` agree on `Final report`,
/// which is what the one shared step should be called. Names that share nothing
/// fall back to the first, because a group still has to be called something.
fn shared_name(names: &[String]) -> String {
    let Some(first) = names.first() else {
        return String::new();
    };
    let mut shared = first.clone();
    for name in &names[1..] {
        let common = shared
            .chars()
            .zip(name.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect::<String>();
        shared = common;
    }
    let trimmed = shared.trim_end_matches([' ', '(', '-', '·', ':']).trim();
    if trimmed.is_empty() {
        first.clone()
    } else {
        trimmed.to_string()
    }
}
