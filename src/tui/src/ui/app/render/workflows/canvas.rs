//! The graph canvas: node boxes, routed wires, and the cursor.
//!
//! Geometry only lives here; the *ordering* — which node sits in which layer and
//! lane — is the SDK's ([`medulla::ui::workflows::GraphLayout`]). This module
//! turns that ordering into cells: a box per node, a wire per edge, and a
//! viewport that scrolls to keep the cursor visible.
//!
//! Wires are routed rather than drawn straight. An edge leaves its source's
//! right edge into the gutter after that layer, runs vertically to its target's
//! lane, and comes back in horizontally — which is legible for a fan-out and
//! survives an edge that spans several layers, because the painter tunnels
//! behind any box in the way rather than writing over it.

use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use medulla::ui::workflows::{GraphLayout, PlacedNode, RunOverlay};

use super::super::super::types::App;
use super::paint::{Canvas, CellStyle};
use super::{LANE_STRIDE, LAYER_STRIDE, NODE_HEIGHT, NODE_WIDTH};

/// The column inside a layer's gutter that wires run vertically in.
///
/// Near the source side, leaving the rest of the gutter for the port label —
/// which is written on the *incoming* run rather than the outgoing one, because
/// every edge out of one node shares that node's exit row and two labels there
/// would overwrite each other.
const GUTTER_COLUMN: usize = NODE_WIDTH + 3;

/// How many columns a port label has, between the gutter's vertical run and the
/// arrowhead at the target's left edge.
const LABEL_WIDTH: usize = LAYER_STRIDE - GUTTER_COLUMN - 2;

/// The row inside a node's box that wires attach to.
const ATTACH_ROW: usize = 2;

impl App {
    /// Draw the graph of the selected workflow.
    pub(super) fn draw_workflow_canvas(&mut self, f: &mut Frame, area: Rect) {
        let focused = matches!(
            self.wf.focus,
            super::super::super::types::WorkflowFocus::Canvas
        );
        let block = crate::ui::widgets::panel(&self.theme, self.canvas_title(), focused);
        let inner = block.inner(area);
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
        self.paint_edges(&mut canvas, layout);
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
                "Write one to .medulla/workflows/<id>.json. The copilot beside",
                "this pane can then edit it — but it acts on a selected",
                "workflow, so there must be one on disk first.",
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

    /// Paint one box per visible node.
    fn paint_nodes(&self, canvas: &mut Canvas, layout: &GraphLayout, overlay: Option<&RunOverlay>) {
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

            let inner = NODE_WIDTH - 2;
            canvas.text(x, y, &format!("╭{}╮", "─".repeat(inner)), style);
            canvas.text(
                x,
                y + 1,
                &format!("│{}│", pad(&format!("{} {}", node.glyph, node.name), inner)),
                style,
            );
            let mark = run.as_ref().map(|state| state.state.glyph()).unwrap_or(" ");
            let summary = if node.summary.is_empty() {
                node.kind.clone()
            } else {
                node.summary.clone()
            };
            canvas.text(
                x,
                y + 2,
                &format!("│{}{mark}│", pad(&summary, inner.saturating_sub(1))),
                style,
            );
            canvas.text(
                x,
                y + NODE_HEIGHT - 1,
                &format!("╰{}╯", "─".repeat(inner)),
                style,
            );
        }
    }

    /// Route and paint one wire per edge whose ends are both placed.
    fn paint_edges(&self, canvas: &mut Canvas, layout: &GraphLayout) {
        for edge in &layout.edges {
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
            let style = if edge.is_back_edge() {
                CellStyle::colored(Color::Yellow).dimmed()
            } else {
                CellStyle::colored(Color::DarkGray)
            };

            let exit = fx + NODE_WIDTH;
            let gutter = fx + GUTTER_COLUMN;
            let entry = tx.saturating_sub(1);
            let (from_row, to_row) = (fy + ATTACH_ROW, ty + ATTACH_ROW);

            canvas.horizontal(exit, gutter, from_row, style);
            if from_row != to_row {
                canvas.vertical(gutter, from_row, to_row, style);
            }
            if gutter < entry {
                canvas.horizontal(gutter, entry, to_row, style);
            } else if gutter > entry {
                // The target is left of the source: the wire comes back the
                // other way, into the box's right edge.
                canvas.horizontal(tx + NODE_WIDTH, gutter, to_row, style);
            }
            let (head_x, head) = if gutter <= entry {
                (entry, '▶')
            } else {
                (tx + NODE_WIDTH, '◀')
            };
            canvas.arrow(head_x, to_row, head, style);

            // The port label is what tells a reader which arm of a branch they
            // are following, so it is written along the run into the target —
            // one label per target row, rather than every arm of a branch
            // fighting for the source's single exit row. Clipped rather than
            // dropped when a case name is long: its first letters still
            // distinguish it from its siblings.
            if let Some(label) = &edge.label {
                if gutter < entry {
                    canvas.text(
                        gutter + 1,
                        to_row,
                        &crate::ui::util::clip(label, LABEL_WIDTH),
                        style,
                    );
                }
            }
        }
    }

    /// The top-left cell of `node`'s box, or `None` when it is scrolled out.
    ///
    /// A node partially off the right or bottom edge still gets a position: the
    /// painter clips it, and half a box at the edge is what tells the operator
    /// the graph continues.
    fn cell_of(&self, node: &PlacedNode) -> Option<(usize, usize)> {
        let layer = node.layer.checked_sub(self.wf.canvas_layer)?;
        let lane = node.lane.checked_sub(self.wf.canvas_lane)?;
        Some((layer * LAYER_STRIDE, lane * LANE_STRIDE))
    }
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

/// Map a colour name from the SDK's vocabulary to a ratatui colour.
fn color_named(name: &str) -> Color {
    super::super::color(name)
}
