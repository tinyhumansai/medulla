//! Unit tests for the workflow graph: topology, simulation stability, and the
//! character drawing surface.

use super::charmap::{elbow_path, point_along, CharMap};
use super::types::{EdgeKind, Graph, NodeKind};
use super::{layout, mock};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;

/// Stand-in colors for the drawing tests, where only the channels matter.
const WHITE: (f64, f64, f64) = (255.0, 255.0, 255.0);
const RED: (f64, f64, f64) = (200.0, 0.0, 0.0);
const BLUE: (f64, f64, f64) = (0.0, 0.0, 200.0);

#[test]
fn mock_graph_fans_in_and_out_of_one_hub() {
    let graph = mock::mock_graph();
    assert_eq!(graph.nodes[0].kind, NodeKind::Orchestrator);
    let feeds = graph
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Feed)
        .count();
    assert!(feeds >= 4, "several input sources feed the hub");
    assert!(
        graph
            .edges
            .iter()
            .all(|e| e.kind != EdgeKind::Feed || e.to == 0),
        "every feed lands on the orchestrator"
    );
    assert!(
        graph.edges.iter().any(|e| e.kind == EdgeKind::Feedback),
        "the graph has at least one loop folding back upstream"
    );
    // Sources sit left of the hub, spawned work right of it: that split is the
    // whole readability of the fan.
    let hub_x = graph.nodes[0].anchor.0;
    for node in &graph.nodes {
        match node.kind {
            NodeKind::Source => assert!(node.anchor.0 < hub_x, "{} is an input", node.label),
            NodeKind::Orchestrator => {}
            _ => assert!(node.anchor.0 > hub_x, "{} is downstream", node.label),
        }
    }
}

#[test]
fn every_node_is_reachable_from_the_hub_or_feeds_it() {
    let graph = mock::mock_graph();
    for (i, node) in graph.nodes.iter().enumerate() {
        let (from, to) = if node.kind == NodeKind::Source {
            (i, 0)
        } else {
            (0, i)
        };
        assert!(directed_path_exists(&graph, from, to), "{} is reachable", node.label);
    }
}

/// Whether following edge direction can reach `to` from `from`.
fn directed_path_exists(graph: &Graph, from: usize, to: usize) -> bool {
    let mut pending = vec![from];
    let mut seen = vec![false; graph.nodes.len()];
    while let Some(node) = pending.pop() {
        if node == to {
            return true;
        }
        if std::mem::replace(&mut seen[node], true) {
            continue;
        }
        pending.extend(graph.edges.iter().filter(|edge| edge.from == node).map(|edge| edge.to));
    }
    false
}

#[test]
fn simulation_wiggles_without_drifting_away() {
    let mut graph = mock::mock_graph();
    let start: Vec<_> = graph.nodes.iter().map(|n| n.pos).collect();
    for _ in 0..600 {
        layout::step(&mut graph);
    }
    let mut moved = 0;
    for (node, from) in graph.nodes.iter().zip(&start) {
        let drift =
            ((node.pos.0 - node.anchor.0).powi(2) + (node.pos.1 - node.anchor.1).powi(2)).sqrt();
        assert!(
            drift < 0.35,
            "{} stays near its anchor (drifted {drift})",
            node.label
        );
        assert!(node.pos.0.is_finite() && node.pos.1.is_finite());
        if (node.pos.0 - from.0).abs() > 1e-6 || (node.pos.1 - from.1).abs() > 1e-6 {
            moved += 1;
        }
    }
    assert!(
        moved > graph.nodes.len() / 2,
        "the graph is actually moving"
    );
}

#[test]
fn the_hub_holds_the_centre() {
    let mut graph = mock::mock_graph();
    for _ in 0..300 {
        layout::step(&mut graph);
    }
    let hub = &graph.nodes[0];
    let drift = ((hub.pos.0 - hub.anchor.0).powi(2) + (hub.pos.1 - hub.anchor.1).powi(2)).sqrt();
    assert!(
        drift < 0.08,
        "the orchestrator barely moves (drifted {drift})"
    );
}

#[test]
fn animation_is_deterministic() {
    let run = || {
        let mut graph = Graph::default();
        for _ in 0..120 {
            layout::step(&mut graph);
        }
        graph.nodes.iter().map(|n| n.pos).collect::<Vec<_>>()
    };
    assert_eq!(
        run(),
        run(),
        "the same frame always renders the same picture"
    );
}

#[test]
fn an_edge_between_two_rows_is_drawn_as_a_right_angled_elbow() {
    let area = Rect::new(0, 0, 12, 4);
    let mut map = CharMap::for_area(area);
    assert_eq!((map.width(), map.height()), (12, 8));
    // Left to right, descending two rows: out, down, in.
    map.elbow((0.0, 0.0), (11.0, 4.0), 0.5, (WHITE, WHITE));
    let mut buf = Buffer::empty(area);
    (&map).render(area, &mut buf);
    let row: String = (0..12).map(|x| buf[(x, 0)].symbol()).collect();
    assert_eq!(
        row, "──────┐     ",
        "out from the left node, then a corner down"
    );
    assert_eq!(buf[(6, 1)].symbol(), "│", "the riser");
    let row: String = (0..12).map(|x| buf[(x, 2)].symbol()).collect();
    assert_eq!(
        row, "      └─────",
        "a corner back out, then in to the right node"
    );
}

#[test]
fn an_edge_within_one_row_is_drawn_straight() {
    let area = Rect::new(0, 0, 8, 2);
    let mut map = CharMap::for_area(area);
    // Less than a row apart: a riser here would be shorter than a cell, and its
    // two corners would collapse into one meaningless junction.
    map.elbow((0.0, 0.0), (7.0, 1.0), 0.5, (WHITE, WHITE));
    let mut buf = Buffer::empty(area);
    (&map).render(area, &mut buf);
    let row: String = (0..8).map(|x| buf[(x, 0)].symbol()).collect();
    assert_eq!(row, "────────");
}

#[test]
fn crossing_edges_merge_into_a_junction() {
    let area = Rect::new(0, 0, 9, 6);
    let mut map = CharMap::for_area(area);
    map.elbow((0.0, 4.0), (8.0, 4.0), 0.5, (WHITE, WHITE));
    map.elbow((4.0, 0.0), (4.0, 10.0), 0.5, (WHITE, WHITE));
    let mut buf = Buffer::empty(area);
    (&map).render(area, &mut buf);
    assert_eq!(buf[(4, 2)].symbol(), "┼", "the two runs cross");
}

#[test]
fn a_node_wins_the_cell_it_shares_with_an_edge() {
    let area = Rect::new(0, 0, 6, 2);
    let mut map = CharMap::for_area(area);
    map.elbow((0.0, 0.0), (5.0, 0.0), 0.5, (BLUE, BLUE));
    map.node((2.0, 0.0), '\u{25cf}', RED, false);
    // And an edge drawn afterwards still may not cover it.
    map.elbow((0.0, 0.0), (5.0, 0.0), 0.5, (BLUE, BLUE));
    let mut buf = Buffer::empty(area);
    (&map).render(area, &mut buf);
    assert_eq!(buf[(2, 0)].symbol(), "\u{25cf}");
    assert_eq!(
        buf[(2, 0)].fg,
        Color::Rgb(200, 0, 0),
        "the node, not the edge"
    );
}

#[test]
fn a_packet_brightens_the_line_without_breaking_it() {
    let area = Rect::new(0, 0, 6, 2);
    let mut map = CharMap::for_area(area);
    map.elbow(
        (0.0, 0.0),
        (5.0, 0.0),
        0.5,
        ((80.0, 80.0, 80.0), (80.0, 80.0, 80.0)),
    );
    map.packet((3.0, 0.0), WHITE);
    let mut buf = Buffer::empty(area);
    (&map).render(area, &mut buf);
    assert_eq!(buf[(3, 0)].symbol(), "─", "the line glyph is kept");
    assert_eq!(buf[(3, 0)].fg, Color::Rgb(255, 255, 255), "and brightened");
    assert_eq!(
        buf[(2, 0)].fg,
        Color::Rgb(80, 80, 80),
        "its neighbours are not"
    );
}

#[test]
fn a_packet_rides_the_elbow_it_was_drawn_for() {
    let path = elbow_path((0.0, 0.0), (10.0, 10.0), 0.5);
    let half = point_along(&path, 0.5);
    assert_eq!(half.0, 5.0, "halfway along is on the riser");
    assert!(
        half.1 > 0.0 && half.1 < 10.0,
        "and partway down it: {half:?}"
    );
    assert_eq!(point_along(&path, 0.0), (0.0, 0.0));
    assert_eq!(point_along(&path, 1.0), (10.0, 10.0));
}

#[test]
fn drawing_off_the_surface_is_dropped() {
    let area = Rect::new(0, 0, 4, 2);
    let mut map = CharMap::for_area(area);
    // Off every edge, including the negative ones: dropped, never clamped onto
    // the border, which would draw a false frame around the graph.
    map.node((-9.0, 0.0), '\u{25cf}', RED, false);
    map.node((0.0, -9.0), '\u{25cf}', RED, false);
    map.node((99.0, 99.0), '\u{25cf}', RED, false);
    map.packet((-5.0, -5.0), RED);
    map.elbow((-40.0, -40.0), (-20.0, -20.0), 0.5, (RED, RED));
    let mut buf = Buffer::empty(area);
    (&map).render(area, &mut buf);
    for y in 0..2 {
        for x in 0..4 {
            assert_eq!(buf[(x, y)].symbol(), " ");
        }
    }
}

#[test]
fn an_edge_fades_from_one_end_color_to_the_other() {
    let area = Rect::new(0, 0, 8, 2);
    let mut map = CharMap::for_area(area);
    map.elbow((0.0, 0.0), (7.0, 0.0), 0.5, (RED, BLUE));
    let mut buf = Buffer::empty(area);
    (&map).render(area, &mut buf);
    let Color::Rgb(r, _, b) = buf[(0, 0)].fg else {
        panic!("edges paint true color");
    };
    assert!(r > b, "the near end keeps the from-color");
    let Color::Rgb(r, _, b) = buf[(7, 0)].fg else {
        panic!("edges paint true color");
    };
    assert!(b > r, "the far end keeps the to-color");
}
