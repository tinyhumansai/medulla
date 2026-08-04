//! Rendering for the animated workflow graph on the Overview tab.
//!
//! The panel answers, at a glance, the question the Overview tab exists for:
//! what does a Medulla run actually look like? Sources fan *in* from the left,
//! the orchestrator sits at the centre, and agents fan *out* to the right into
//! subagents, tools, and review loops that fold back on themselves.
//!
//! The topology is currently mock data ([`mock`]); everything else — the force
//! simulation in [`layout`], the character surface in [`charmap`], and the
//! drawing here — is general and will carry a live run unchanged.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::super::super::types::App;
use super::charmap::{elbow_path, mix, point_along, rgb, scale, CharMap, UNITS_H, UNITS_W};
use super::{layout, Graph, NodeKind};

/// Columns kept clear on the left and right of the canvas, for the labels of
/// the nodes pressed against those edges. Sized for the longest label the mock
/// graph carries plus its glyph and separating space.
const LABEL_GUTTER: f64 = 11.0;
/// Rows kept clear above and below the canvas, so a node hard against the top
/// or bottom still has a whole row for its label.
const EDGE_ROWS: f64 = 0.5;
/// How much of the available canvas the graph actually spreads across, centred
/// in what is left.
///
/// Below 1.0 on purpose: a fan stretched to the full width of a wide terminal
/// turns every edge into a long empty corridor, and the run reads as a cluster
/// of connected work rather than as a scatter when it is drawn tighter than the
/// space allows.
const SPREAD: f64 = 0.72;
/// How fast a packet travels the length of an edge, in edge-lengths per second.
const PACKET_SPEED: f64 = 0.28;
/// How far an edge's color is dimmed relative to the nodes it joins, so the
/// wiring stays behind the things it is wiring together.
const EDGE_DIM: f64 = 0.62;
/// How far either side of halfway an edge's vertical run may be nudged, to keep
/// the risers of a bundle of parallel edges out of one another's column.
const RISER_SPREAD: f64 = 0.18;

impl App {
    /// Draw the workflow graph panel, advancing its simulation by one frame.
    ///
    /// Stepping the simulation from the draw path is intentional: the panel's
    /// only clock is the render tick, and the graph owns no state that anything
    /// else reads. Below a few rows the canvas cannot say anything useful, so a
    /// cramped panel gets a plain note instead of an unreadable smear.
    pub(super) fn draw_overview_graph(&mut self, f: &mut Frame, area: Rect) {
        let block = self.panel("Workflow");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let drawable_width = inner.width as f64 - 1.0 - 2.0 * LABEL_GUTTER;
        let drawable_height = inner.height.saturating_sub(1) as f64 * UNITS_H as f64
            - 1.0
            - 2.0 * EDGE_ROWS * UNITS_H as f64;
        if drawable_width * SPREAD < 8.0 || drawable_height * SPREAD < 4.0 {
            f.render_widget(
                Paragraph::new(TLine::from(Span::styled(
                    "Workflow graph needs a larger window.",
                    Style::default().add_modifier(Modifier::DIM),
                ))),
                inner,
            );
            return;
        }

        layout::step(&mut self.graph);

        // The legend is worth a row of the canvas: without it the glyphs are
        // just shapes, and the panel is the operator's first look at the model.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(1)])
            .split(inner);
        let canvas = rows[0];

        let mut map = CharMap::for_area(canvas);
        let positions = positions_on(&self.graph, &map);
        draw_edges(&mut map, &self.graph, &positions);
        draw_nodes(&mut map, &self.graph, &positions);
        (&map).render(canvas, f.buffer_mut());
        draw_labels(f, canvas, &self.graph, &positions);

        f.render_widget(Paragraph::new(legend()), rows[1]);
    }
}

/// Project every node from normalised graph space into canvas coordinates.
fn positions_on(graph: &Graph, map: &CharMap) -> Vec<(f64, f64)> {
    // Margins are given in cells; the pixel grid is finer than that on both
    // axes, so each one is converted before it is subtracted.
    let (pad_x, pad_y) = (LABEL_GUTTER * UNITS_W as f64, EDGE_ROWS * UNITS_H as f64);
    let full_x = (map.width() as f64 - 1.0 - pad_x * 2.0).max(1.0);
    let full_y = (map.height() as f64 - 1.0 - pad_y * 2.0).max(1.0);
    let (span_x, span_y) = (full_x * SPREAD, full_y * SPREAD);
    // Whatever the compaction gave back becomes even margin on both sides.
    let (pad_x, pad_y) = (
        pad_x + (full_x - span_x) / 2.0,
        pad_y + (full_y - span_y) / 2.0,
    );
    graph
        .nodes
        .iter()
        .map(|node| {
            (
                pad_x + (node.pos.0 + 1.0) / 2.0 * span_x,
                pad_y + (node.pos.1 + 1.0) / 2.0 * span_y,
            )
        })
        .collect()
}

/// Convert graph y (up-positive, as the anchors read) to pixel y (down-positive).
///
/// Done at draw time rather than in the anchors so the mock table stays in the
/// orientation a person would sketch it in.
fn flip(map_height: usize, y: f64) -> f64 {
    map_height as f64 - 1.0 - y
}

/// Draw every edge as a color-interpolated line, then its travelling packet.
///
/// Each edge carries one moving highlight. It is the cheapest possible signal
/// that work is flowing rather than merely connected, and it gives the panel
/// motion even when the simulation has settled.
fn draw_edges(map: &mut CharMap, graph: &Graph, positions: &[(f64, f64)]) {
    let height = map.height();
    let t = graph.time();
    for (i, edge) in graph.edges.iter().enumerate() {
        let (x0, y0) = positions[edge.from];
        let (x1, y1) = positions[edge.to];
        let (from, to) = ((x0, flip(height, y0)), (x1, flip(height, y1)));
        let edge_rgb = edge.kind.rgb();
        // Fade the line toward the node colors at each end, so an edge reads as
        // belonging to the two things it joins.
        let from_rgb = mix(edge_rgb, graph.nodes[edge.from].kind.rgb(), 0.35);
        let to_rgb = mix(edge_rgb, graph.nodes[edge.to].kind.rgb(), 0.35);
        // An irrational stride over the edge index spreads the risers evenly
        // however many edges share a direction.
        let bias = 0.5 + RISER_SPREAD * ((i as f64 * 0.618_034).fract() * 2.0 - 1.0);
        map.elbow(
            from,
            to,
            bias,
            (scale(from_rgb, EDGE_DIM), scale(to_rgb, EDGE_DIM)),
        );
    }

    // Packets are a higher layer than edges, so all wires must exist before a
    // packet is placed. Otherwise a later crossing edge loses connections at
    // the packet cell.
    for (i, edge) in graph.edges.iter().enumerate() {
        let (x0, y0) = positions[edge.from];
        let (x1, y1) = positions[edge.to];
        let (from, to) = ((x0, flip(height, y0)), (x1, flip(height, y1)));
        let edge_rgb = edge.kind.rgb();
        let to_rgb = mix(edge_rgb, graph.nodes[edge.to].kind.rgb(), 0.35);
        let bias = 0.5 + RISER_SPREAD * ((i as f64 * 0.618_034).fract() * 2.0 - 1.0);
        // Packets are spaced by an irrational stride so they never form a
        // visible pulse across the whole graph at once. They ride the drawn
        // elbow rather than the straight line between the two nodes, or they
        // would drift off their own wire.
        let progress = (t * PACKET_SPEED + i as f64 * 0.618_034).fract();
        let at = point_along(&elbow_path(from, to, bias), progress);
        map.packet(at, mix(to_rgb, (255.0, 255.0, 255.0), 0.5));
    }
}

/// Draw every node as its marker glyph, breathing gently.
///
/// The pulse is brightness only. Nodes are single characters now, so there is no
/// size to animate, and a node that faded far down would read as having gone
/// away rather than as idling.
fn draw_nodes(map: &mut CharMap, graph: &Graph, positions: &[(f64, f64)]) {
    let height = map.height();
    let t = graph.time();
    for (node, &(x, y)) in graph.nodes.iter().zip(positions) {
        let pulse = 0.80 + 0.20 * (t * 1.4 + node.phase).sin();
        map.node(
            (x, flip(height, y)),
            node.kind.glyph(),
            scale(node.kind.rgb(), pulse),
            node.kind == NodeKind::Orchestrator,
        );
    }
}

/// Print each node's name beside its marker.
///
/// The marker itself is drawn on the canvas, so a label is only the name — and
/// it is written straight into the frame buffer rather than as a widget because
/// it is positioned per-node rather than on any grid the layout engine knows.
fn draw_labels(f: &mut Frame, area: Rect, graph: &Graph, positions: &[(f64, f64)]) {
    let height = area.height as usize * UNITS_H;
    for (node, &(x, y)) in graph.nodes.iter().zip(positions) {
        let y = flip(height, y);
        let (col, row) = (
            (x / UNITS_W as f64).round() as i64,
            (y / UNITS_H as f64).round() as i64,
        );
        let style = Style::default()
            .fg(rgb(node.kind.rgb()))
            .bg(Color::Reset)
            .add_modifier(if node.kind == NodeKind::Orchestrator {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });

        // The hub gets its name centred underneath so nothing crowds the middle
        // of the fan, where the most edges cross.
        // Every label is padded with a blank on the side that faces the graph,
        // so an edge running into the text stops a cell short of it instead of
        // touching the first letter.
        let (text, start, row) = if node.kind == NodeKind::Orchestrator {
            let text = format!(" {} ", node.label);
            let width = text.width() as i64;
            (text, col - width / 2, row + 1)
        } else if node.label_left {
            let text = format!(" {} ", node.label);
            let width = text.width() as i64;
            (text, col - width, row)
        } else {
            (format!(" {} ", node.label), col + 1, row)
        };
        put(f, area, start, row, &text, style);
    }
}

/// Write `text` at cell `(col, row)` relative to `area`, clipped to it.
fn put(f: &mut Frame, area: Rect, col: i64, row: i64, text: &str, style: Style) {
    if row < 0 || row >= area.height as i64 {
        return;
    }
    let buf = f.buffer_mut();
    let mut col = col;
    for ch in text.chars() {
        if col >= area.width as i64 {
            break;
        }
        if col >= 0 {
            if let Some(cell) = buf.cell_mut((area.x + col as u16, area.y + row as u16)) {
                cell.set_char(ch).set_style(style);
            }
        }
        col += ch.to_string().width().max(1) as i64;
    }
}

/// The one-line key to the node glyphs, drawn under the canvas.
fn legend() -> TLine<'static> {
    let entry = |kind: NodeKind, name: &'static str| {
        vec![
            Span::styled(
                format!("{} ", kind.glyph()),
                Style::default().fg(rgb(kind.rgb())),
            ),
            Span::styled(name, Style::default().add_modifier(Modifier::DIM)),
            Span::raw("  "),
        ]
    };
    TLine::from(
        [
            entry(NodeKind::Source, "sources"),
            entry(NodeKind::Orchestrator, "orchestrator"),
            entry(NodeKind::Agent, "agents"),
            entry(NodeKind::Subagent, "subagents"),
            entry(NodeKind::Loop, "loops"),
            entry(NodeKind::Sink, "output"),
        ]
        .concat(),
    )
}
