//! The graph canvas: node markers, routed wires, and the cursor.
//!
//! Geometry only lives here; the *ordering* — which node sits in which layer and
//! lane — is the SDK's ([`medulla::ui::workflows::GraphLayout`]). This module
//! turns that ordering into cells: a marker and a label per node, a wire per
//! edge, and a viewport that scrolls to keep the cursor visible.
//!
//! Wires are routed rather than drawn straight. An edge leaves its source's
//! right edge into the gutter after that layer, runs vertically to its target's
//! lane, and comes back in horizontally — which is legible for a fan-out and
//! survives an edge that spans several layers, because the painter tunnels
//! behind any box in the way rather than writing over it.
//!
//! Wires also carry a moving highlight, so a plan reads as something work flows
//! through rather than as a static diagram. With a run overlaid the highlight is
//! literal — it only travels the edges that run actually took — and with no run
//! it is an idle drift that makes the shape of the graph easier to follow.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Color;
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use medulla::ui::workflows::{GraphLayout, PlacedNode, RunOverlay};

use super::super::super::types::App;
use super::paint::{Canvas, CellStyle};
use super::GUTTER_SPAN;

/// Columns of a node's slot that its name is allowed to fill before a wire is
/// routed around it, leaving a gap between the longest label and the wire that
/// leaves it.
const NODE_GAP: usize = 1;

/// Cells past a node's trailing edge that its outgoing wires turn down in.
///
/// Near the source side, leaving the rest of the gutter for the port label —
/// which is written on the *incoming* run rather than the outgoing one, because
/// every edge out of one node shares that node's exit row and two labels there
/// would overwrite each other.
const GUTTER_GAP: usize = NODE_GAP + 1;

/// How many columns a port label has, between the gutter's vertical run and the
/// arrowhead at the target's leading edge.
const LABEL_WIDTH: usize = GUTTER_SPAN - GUTTER_GAP - 2;

/// The row inside a node that wires attach to. A node is one row tall, so this
/// is that row.
const ATTACH_ROW: usize = 0;

/// Radians per frame of the nodes' idle sway. About a nine-second cycle at the
/// app's draw rate — slow enough to read as drift rather than as movement.
const DRIFT_RATE: f64 = 0.06;

/// How many cells the flow highlight advances per drawn frame.
///
/// Well under one: at the ~11fps the app redraws at, a whole cell per frame is
/// a blur rather than a signal.
const FLOW_SPEED: f64 = 0.45;

impl App {
    /// Draw the graph of the selected workflow.
    pub(super) fn draw_workflow_canvas(&mut self, f: &mut Frame, area: Rect) {
        let panes = if area.height >= 16 && self.selected_graph_node().is_some() {
            // Tall enough for every band the fold produced plus the panel's own
            // borders, and no taller: a graph that reserved more would spend the
            // difference on blank canvas while the inspector under it went
            // short. Past half the pane the canvas scrolls instead of growing —
            // the inspector is not worth starving for a graph nobody asked to
            // see all of at once.
            let graph_height = (self.folded_rows() as u16 + 2).max(5).min(area.height / 2);
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(graph_height), Constraint::Min(8)])
                .split(area)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0), Constraint::Length(0)])
                .split(area)
        };
        self.draw_workflow_graph(f, panes[0]);
        if panes[1].height > 0 {
            self.draw_workflow_node_preview(f, panes[1]);
        }
    }

    /// Draw only the graph panel, leaving pane allocation to the canvas view.
    fn draw_workflow_graph(&mut self, f: &mut Frame, area: Rect) {
        let focused = matches!(
            self.wf.focus,
            super::super::super::types::WorkflowFocus::Canvas
        );
        let block = crate::ui::widgets::panel(&self.theme, self.canvas_title(), focused);
        let inner = block.inner(area);
        self.wf.graph_rows = inner.height as usize;
        f.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let layout = self.workflow_layout();
        if layout.nodes.is_empty() {
            f.render_widget(Paragraph::new(Text::from(self.empty_canvas_lines())), inner);
            return;
        }

        let overlay = self.selected_workflow_run().map(RunOverlay::new);
        let mut canvas = Canvas::new(inner.width as usize, inner.height as usize);
        // Wires first, so a box drawn over a wire's last cell wins — the
        // painter locks node cells, and an edge routed before its target exists
        // would otherwise have written into it.
        self.paint_edges(&mut canvas, layout, overlay.as_ref());
        self.paint_nodes(&mut canvas, layout, overlay.as_ref());
        f.render_widget(Paragraph::new(Text::from(canvas.into_lines())), inner);
    }

    /// The canvas panel's title: the workflow, and what is overlaid on it.
    fn canvas_title(&self) -> String {
        let Some(workflow) = self.selected_workflow() else {
            return "Graph".to_string();
        };
        let layout = self.workflow_layout();
        let mut title = format!(
            "{} · {} node{}",
            workflow.name,
            layout.nodes.len(),
            if layout.nodes.len() == 1 { "" } else { "s" }
        );
        if let Some(run) = self.selected_workflow_run() {
            title.push_str(&format!(
                " · run {} {}",
                medulla::ui::workflows::rows::short_run_id(&run.id),
                medulla::ui::workflows::status_label(run.status)
            ));
        }
        title
    }

    /// What the canvas says when there is no graph to draw.
    fn empty_canvas_lines(&self) -> Vec<TLine<'static>> {
        let dim = ratatui::style::Style::default().add_modifier(ratatui::style::Modifier::DIM);
        let message = if self.workflows.is_empty() {
            vec![
                "No workflows installed.",
                "",
                "Create one with medulla workflow create. It will be saved under",
                "your Medulla home; the copilot beside this pane can then edit",
                "it, but it needs a selected workflow on disk first.",
                "",
                "r re-reads the store.",
            ]
        } else {
            vec![
                "This workflow has no nodes.",
                "",
                "Ask the copilot for a first step, or add one with",
                "medulla workflow apply-ops.",
            ]
        };
        message
            .into_iter()
            .map(|line| TLine::from(Span::styled(line, dim)))
            .collect()
    }

    /// Paint one marker and label per visible node.
    fn paint_nodes(&self, canvas: &mut Canvas, layout: &GraphLayout, overlay: Option<&RunOverlay>) {
        let node_width = self.node_width();
        for (index, node) in layout.nodes.iter().enumerate() {
            let Some((x, y)) = self.cell_of(node) else {
                continue;
            };
            let run = overlay.map(|overlay| overlay.node(&node.id));
            // A run recolours the box by how the node fared: reading "which step
            // failed" off the plan is the whole reason for overlaying a run onto
            // it, and the node's kind is still in the glyph.
            let color = match &run {
                Some(state) => color_named(state.state.color()),
                None => color_named(medulla::ui::workflows::graph::color_for_kind(&node.kind)),
            };
            let style = CellStyle::colored(color).selected(index == self.wf.node_index);
            // A node the run never reached is dimmed, so the path the run
            // actually took stands out from the rest of the plan.
            let style = match &run {
                Some(state) if state.state == medulla::ui::workflows::NodeRunState::Pending => {
                    style.dimmed()
                }
                _ => style,
            };

            // A node is its marker, its name, and — under a run — the state
            // glyph. The marker carries the kind and the colour carries the run
            // state, so the box, the border and the kind subtitle the box used
            // to hold are all saying something already said elsewhere.
            //
            // Written at its own length rather than padded out to the column:
            // the wires attach to the text, so padding would only push every
            // connector a few cells away from the name it belongs to.
            let mark = run.as_ref().map(|state| state.state.glyph()).unwrap_or("");
            let label = label_of(node, mark, node_width);
            canvas.text(x, y, &label, style.bold());
        }
    }

    /// Route and paint one wire per edge whose ends are both placed.
    fn paint_edges(&self, canvas: &mut Canvas, layout: &GraphLayout, overlay: Option<&RunOverlay>) {
        let node_width = self.node_width();
        for (index, edge) in layout.edges.iter().enumerate() {
            let (Some(from), Some(to)) = (
                layout.index_of(&edge.from).and_then(|i| layout.node(i)),
                layout.index_of(&edge.to).and_then(|i| layout.node(i)),
            ) else {
                continue;
            };
            let (Some((fx, fy)), Some((tx, ty))) = (self.cell_of(from), self.cell_of(to)) else {
                // Both ends off screen in the same direction means the wire is
                // off screen too; one end off screen still draws the visible
                // part, which is what tells the operator there is more graph
                // that way.
                continue;
            };
            // A back edge is a loop, and a loop drawn as a rightward arrow reads
            // as a mistake in the graph rather than in the drawing.
            //
            // Everything else takes its source node's colour, dimmed: a wire
            // that belongs to the box it leaves is far easier to follow out of a
            // branch than one more grey line among several.
            let wire_color = if edge.is_back_edge() {
                Color::Yellow
            } else {
                color_named(medulla::ui::workflows::graph::color_for_kind(&from.kind))
            };
            let style = CellStyle::colored(wire_color).dimmed();

            // Which way the two bands run. In a reversed band a forward edge
            // travels leftward, so every "out of the source" and "into the
            // target" column below is mirrored rather than assumed rightward.
            let out_reversed = self.layer_reversed(from.layer);
            let in_reversed = self.layer_reversed(to.layer);
            let folds = self.band_of(to.layer) != self.band_of(from.layer);

            // Wires leave and arrive at the *text*, not at the column slot: a
            // connector that started a few cells past the end of a short name
            // is what made the graph look ragged, however well the columns
            // themselves lined up. The column a wire turns down in is still on
            // the slot grid, so the turns of a whole column agree.
            let (from_width, to_width) = (
                self.label_width(from, overlay),
                self.label_width(to, overlay),
            );
            let (exit, gutter) = if out_reversed {
                (fx.saturating_sub(1), fx.saturating_sub(GUTTER_GAP))
            } else {
                (fx + from_width, fx + node_width + GUTTER_GAP)
            };
            // The cell an arrowhead lands on: just outside the target's leading
            // edge, which is its right edge on a band that runs right to left.
            let entry = if in_reversed {
                tx + to_width
            } else {
                tx.saturating_sub(1)
            };
            let head = if in_reversed { '◀' } else { '▶' };
            let (from_row, to_row) = (fy + ATTACH_ROW, ty + ATTACH_ROW);
            // Whether the target is ahead of the source along the band: a
            // target behind it is a loop, and comes back into its trailing edge.
            let forward = if out_reversed { tx < fx } else { tx > fx };

            // The wire as a polyline, corner to corner. Building it up front is
            // what lets the flow highlight ride the path that was actually
            // drawn rather than the straight line between the two nodes, which
            // is not where the wire is.
            let points: Vec<(usize, usize)> = if folds {
                // Out, down into the blank row above the target's band, across
                // to the target's column, then down into it. With alternating
                // band directions that "across" is a short hop, because the
                // next band picks up under where this one ended.
                let link_row = to_row.saturating_sub(1);
                vec![
                    (exit, from_row),
                    (gutter, from_row),
                    (gutter, link_row),
                    (entry, link_row),
                    (entry, to_row),
                ]
            } else if forward {
                vec![
                    (exit, from_row),
                    (gutter, from_row),
                    (gutter, to_row),
                    (entry, to_row),
                ]
            } else {
                // A loop back to an earlier step in the same band: it returns
                // the way it came, into the node's trailing edge.
                let tail = if in_reversed {
                    tx.saturating_sub(1)
                } else {
                    tx + to_width
                };
                vec![
                    (exit, from_row),
                    (gutter, from_row),
                    (gutter, to_row),
                    (tail, to_row),
                ]
            };
            let route = paint_route(canvas, &points, style);

            let (head_x, head) = if folds || forward {
                (entry, head)
            } else if in_reversed {
                (tx.saturating_sub(1), '▶')
            } else {
                (tx + to_width, '◀')
            };
            canvas.arrow(head_x, to_row, head, style);

            // The moving highlight. Undimmed against the dim wire, so the eye
            // follows it without the wire itself competing with the nodes.
            if let Some((x, y)) = self.flow_cell(index, &route, overlay, edge) {
                canvas.pulse(x, y, brightened(wire_color));
            }

            // The port label is what tells a reader which arm of a branch they
            // are following, so it is written along the run into the target —
            // one label per target row, rather than every arm of a branch
            // fighting for the source's single exit row. Clipped rather than
            // dropped when a case name is long: its first letters still
            // distinguish it from its siblings.
            if let Some(label) = &edge.label {
                let label = crate::ui::util::clip(label, LABEL_WIDTH);
                // Written along the run into the target — one label per target
                // row, rather than every arm of a branch fighting for the
                // source's single exit row. A folded arm writes its name on the
                // row it comes across on instead; dropping it there would leave
                // the reader unable to tell which arm of a branch wrapped, which
                // is the one thing the label exists to say.
                let (label_x, label_row) = if folds {
                    (gutter.min(entry) + 1, to_row.saturating_sub(1))
                } else if out_reversed {
                    (entry + 1, to_row)
                } else {
                    (gutter + 1, to_row)
                };
                if forward || folds {
                    canvas.text(label_x, label_row, &label, style);
                }
            }
        }
    }

    /// How many columns a node's label actually occupies on screen.
    ///
    /// The wires attach here, so this has to agree exactly with what
    /// [`paint_nodes`](Self::paint_nodes) drew — hence both going through
    /// [`label_of`].
    fn label_width(&self, node: &PlacedNode, overlay: Option<&RunOverlay>) -> usize {
        let mark = overlay
            .map(|overlay| overlay.node(&node.id).state.glyph())
            .unwrap_or("");
        label_of(node, mark, self.node_width()).width()
    }

    /// Where this edge's flow highlight sits on `route` this frame, if it is
    /// flowing at all.
    ///
    /// With a run overlaid only the edges that run actually travelled flow — a
    /// highlight on a branch the run never took would claim something untrue.
    /// Edges are offset from one another by an irrational stride so a fan-out
    /// does not pulse in lockstep.
    fn flow_cell(
        &self,
        index: usize,
        route: &[(usize, usize)],
        overlay: Option<&RunOverlay>,
        edge: &medulla::ui::workflows::PlacedEdge,
    ) -> Option<(usize, usize)> {
        if route.is_empty() {
            return None;
        }
        if let Some(overlay) = overlay {
            let source = overlay.node(&edge.from).state;
            let target = overlay.node(&edge.to).state;
            let reached = |state| state != medulla::ui::workflows::NodeRunState::Pending;
            if !reached(source) || !reached(target) {
                return None;
            }
        }
        let offset = (index as f64 * 0.618_034).fract();
        let step = (self.frame as f64 * FLOW_SPEED + offset * route.len() as f64) as usize;
        Some(route[step % route.len()])
    }

    /// The top-left cell of `node`, or `None` when it is scrolled out.
    ///
    /// The horizontal position comes from the fold, so it is never off the side;
    /// only the vertical scroll can put a node out of view.
    fn cell_of(&self, node: &PlacedNode) -> Option<(usize, usize)> {
        let (x, row) = self.graph_cell(node.layer, node.lane);
        Some((x + self.drift(node), row.checked_sub(self.wf.canvas_row)?))
    }

    /// A slow one-cell sway, so the graph breathes instead of sitting still.
    ///
    /// One cell is the whole budget: the wires are re-routed from these
    /// positions every frame, so a node that wandered further would drag its
    /// wires into its neighbours, and a label that moves while it is being read
    /// is worse than a static diagram. Each node has its own phase, which is
    /// what makes it read as drift rather than as the panel juddering.
    fn drift(&self, node: &PlacedNode) -> usize {
        // The node under the cursor never moves. It is the one an operator is
        // looking at, and it is the anchor everything else is judged against.
        if self.selected_graph_node().map(|selected| &selected.id) == Some(&node.id) {
            return 0;
        }
        let phase = (node.layer * 7 + node.lane * 3) as f64 * 0.9;
        let sway = (self.frame as f64 * DRIFT_RATE + phase).sin();
        usize::from(sway > 0.5)
    }
}

/// A node's drawn label: its marker, its name, and the run's state glyph.
///
/// Clipped to the column width, never padded — the caller draws it at its own
/// length and the wires meet it there.
fn label_of(node: &PlacedNode, mark: &str, width: usize) -> String {
    let label = format!("{} {}", node.glyph, node.name);
    format!("{}{mark}", crate::ui::util::clip(&label, width))
}

/// Clip or pad `text` to exactly `width` display columns.
fn pad(text: &str, width: usize) -> String {
    let mut out: String = crate::ui::util::clip(text, width);
    let len = out.chars().count();
    if len < width {
        out.push_str(&" ".repeat(width - len));
    }
    out
}

/// Paint a polyline as axis-aligned runs, returning the cells it covers in
/// flow order.
///
/// Every segment must be horizontal or vertical; a diagonal is a bug in the
/// caller's routing, and is dropped rather than approximated.
fn paint_route(
    canvas: &mut Canvas,
    points: &[(usize, usize)],
    style: CellStyle,
) -> Vec<(usize, usize)> {
    let mut route: Vec<(usize, usize)> = Vec::new();
    for pair in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        if y0 == y1 {
            canvas.horizontal(x0, x1, y0, style);
            let step: Box<dyn Iterator<Item = usize>> = if x0 <= x1 {
                Box::new(x0..=x1)
            } else {
                Box::new((x1..=x0).rev())
            };
            route.extend(step.map(|x| (x, y0)));
        } else if x0 == x1 {
            canvas.vertical(x0, y0, y1, style);
            let step: Box<dyn Iterator<Item = usize>> = if y0 <= y1 {
                Box::new(y0..=y1)
            } else {
                Box::new((y1..=y0).rev())
            };
            route.extend(step.map(|y| (x0, y)));
        }
    }
    route
}

/// The light variant of a colour, for the flow highlight.
///
/// The wire is drawn dim in its own colour, so the highlight has to be more than
/// the same colour undimmed to be visible at a glance — but it also has to stay
/// recognisably that wire's colour, or it reads as a separate thing crawling
/// over the graph. The light variant is both. Colours with no lighter twin fall
/// back to white, which is still clearly a highlight.
fn brightened(color: Color) -> Color {
    match color {
        Color::Black | Color::DarkGray => Color::Gray,
        Color::Red => Color::LightRed,
        Color::Green => Color::LightGreen,
        Color::Yellow => Color::LightYellow,
        Color::Blue => Color::LightBlue,
        Color::Magenta => Color::LightMagenta,
        Color::Cyan => Color::LightCyan,
        Color::Gray => Color::White,
        other => other,
    }
}

/// Map a colour name from the SDK's vocabulary to a ratatui colour.
fn color_named(name: &str) -> Color {
    super::super::color(name)
}
