//! Tests for the terminal graph layout and its cursor.

use super::*;
use serde_json::json;
use tinyflows::model::{Edge, Port, Position};

/// A node of `kind` with `id`, carrying `config`.
fn node(id: &str, kind: NodeKind, config: serde_json::Value) -> Node {
    Node {
        id: id.to_string(),
        kind,
        type_version: 1,
        name: format!("{id} step"),
        config,
        ports: Vec::new(),
        position: None,
    }
}

/// An edge from `from`'s `port` to `to`'s `main`.
fn edge(from: &str, port: &str, to: &str) -> Edge {
    Edge {
        from_node: from.to_string(),
        from_port: port.to_string(),
        to_node: to.to_string(),
        to_port: "main".to_string(),
    }
}

/// trigger → check, then check's two ports to `yes` and `no`, which both feed
/// `join`. A diamond: two layers of branch, one barrier.
fn diamond() -> WorkflowGraph {
    WorkflowGraph {
        nodes: vec![
            node(
                "trigger",
                NodeKind::Trigger,
                json!({"trigger_kind":"manual"}),
            ),
            node("check", NodeKind::Condition, json!({"expression":"=.ok"})),
            node("yes", NodeKind::Agent, json!({"prompt":"do the thing"})),
            node("no", NodeKind::Agent, json!({"prompt":"complain"})),
            node("join", NodeKind::Merge, json!({"inputs":["a","b"]})),
        ],
        edges: vec![
            edge("trigger", "main", "check"),
            edge("check", "true", "yes"),
            edge("check", "false", "no"),
            edge("yes", "main", "join"),
            edge("no", "main", "join"),
        ],
        ..Default::default()
    }
}

#[test]
fn a_chain_gets_one_node_per_layer_in_order() {
    let graph = WorkflowGraph {
        nodes: vec![
            node("a", NodeKind::Trigger, json!({})),
            node("b", NodeKind::Agent, json!({})),
            node("c", NodeKind::Transform, json!({})),
        ],
        edges: vec![edge("a", "main", "b"), edge("b", "main", "c")],
        ..Default::default()
    };

    let layout = GraphLayout::build(&graph);

    assert_eq!(layout.layers, 3);
    assert_eq!(layout.lanes, 1);
    let layers: Vec<usize> = layout.nodes.iter().map(|node| node.layer).collect();
    assert_eq!(layers, vec![0, 1, 2]);
}

#[test]
fn a_branch_puts_its_arms_in_separate_lanes_of_one_layer() {
    let layout = GraphLayout::build(&diamond());

    let arms = layout.layer_nodes(2);
    assert_eq!(arms.len(), 2, "both arms share a layer");
    assert_eq!(arms[0].lane, 0);
    assert_eq!(arms[1].lane, 1);
    // The barrier waits for both, so it cannot sit beside them.
    assert_eq!(
        layout.node(layout.index_of("join").unwrap()).unwrap().layer,
        3
    );
}

#[test]
fn a_node_never_sits_left_of_anything_that_feeds_it() {
    // `late` is written first but fed by `early`, so document order must not
    // decide the layering.
    let graph = WorkflowGraph {
        nodes: vec![
            node("late", NodeKind::Agent, json!({})),
            node("early", NodeKind::Trigger, json!({})),
        ],
        edges: vec![edge("early", "main", "late")],
        ..Default::default()
    };

    let layout = GraphLayout::build(&graph);

    let early = layout
        .node(layout.index_of("early").unwrap())
        .unwrap()
        .layer;
    let late = layout.node(layout.index_of("late").unwrap()).unwrap().layer;
    assert!(early < late, "{early} should be left of {late}");
}

#[test]
fn nodes_are_returned_in_layer_then_lane_reading_order() {
    let layout = GraphLayout::build(&diamond());

    let order: Vec<(usize, usize)> = layout
        .nodes
        .iter()
        .map(|node| (node.layer, node.lane))
        .collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);
}

#[test]
fn a_cycle_still_lays_out_instead_of_hanging() {
    let graph = WorkflowGraph {
        nodes: vec![
            node("a", NodeKind::Trigger, json!({})),
            node("b", NodeKind::Agent, json!({})),
        ],
        edges: vec![edge("a", "main", "b"), edge("b", "main", "a")],
        ..Default::default()
    };

    let layout = GraphLayout::build(&graph);

    assert_eq!(layout.nodes.len(), 2);
    assert!(
        layout.edges.iter().any(PlacedEdge::is_back_edge),
        "the closing edge is a back edge and should say so"
    );
}

#[test]
fn a_loop_is_as_deep_as_its_body_not_as_deep_as_the_relaxation_ran() {
    // A trigger into a five-step loop: the plan is six columns deep, and the
    // arm that closes the loop says nothing about depth.
    let body = ["one", "two", "three", "four", "five"];
    let mut nodes = vec![node(
        "start",
        NodeKind::Trigger,
        json!({"trigger_kind":"manual"}),
    )];
    nodes.extend(body.iter().map(|id| node(id, NodeKind::Agent, json!({}))));
    let mut edges = vec![edge("start", "main", "one")];
    edges.extend(
        body.windows(2)
            .map(|pair| edge(pair[0], "main", pair[1]))
            // …and back to the top of the body, which is the loop.
            .chain(std::iter::once(edge("five", "main", "one"))),
    );

    let layout = GraphLayout::build(&WorkflowGraph {
        nodes,
        edges,
        ..Default::default()
    });

    // Six, not "however far one relaxation pass per node pushed the cycle".
    // Left in, the closing arm advanced every member of the loop on every pass
    // and this came out at 26 layers — a fold of mostly empty bands, each of
    // which the renderer then had to find room for.
    assert_eq!(layout.layers, 6, "{:?}", layout.nodes);
    assert_eq!(
        layout.nodes.iter().map(|node| node.layer).max(),
        Some(5),
        "no node sits past the last column"
    );
}

#[test]
fn every_node_of_a_pure_cycle_still_gets_a_distinct_layer() {
    // No root at all, so the walk has to start somewhere arbitrary. Exactly one
    // edge is removed either way, which is what keeps the ring from collapsing
    // into a single column.
    let ring = ["a", "b", "c"];
    let layout = GraphLayout::build(&WorkflowGraph {
        nodes: ring
            .iter()
            .map(|id| node(id, NodeKind::Agent, json!({})))
            .collect(),
        edges: vec![
            edge("a", "main", "b"),
            edge("b", "main", "c"),
            edge("c", "main", "a"),
        ],
        ..Default::default()
    });

    assert_eq!(layout.layers, 3, "{:?}", layout.nodes);
    let layers: Vec<usize> = layout.nodes.iter().map(|node| node.layer).collect();
    assert_eq!(layers, vec![0, 1, 2]);
}

#[test]
fn a_self_edge_does_not_run_the_relaxation_to_its_limit() {
    let graph = WorkflowGraph {
        nodes: vec![node("a", NodeKind::Agent, json!({}))],
        edges: vec![edge("a", "main", "a")],
        ..Default::default()
    };

    let layout = GraphLayout::build(&graph);

    assert_eq!(layout.nodes[0].layer, 0);
}

#[test]
fn an_edge_naming_a_missing_node_is_dropped_rather_than_panicking() {
    let graph = WorkflowGraph {
        nodes: vec![node("a", NodeKind::Trigger, json!({}))],
        edges: vec![edge("a", "main", "ghost")],
        ..Default::default()
    };

    let layout = GraphLayout::build(&graph);

    assert!(layout.edges.is_empty());
    assert_eq!(layout.nodes.len(), 1);
}

#[test]
fn an_edge_off_a_named_port_carries_that_port_as_its_label() {
    let layout = GraphLayout::build(&diamond());

    let labels: Vec<Option<String>> = layout
        .edges
        .iter()
        .filter(|edge| edge.from == "check")
        .map(|edge| edge.label.clone())
        .collect();
    assert_eq!(
        labels,
        vec![Some("true".to_string()), Some("false".to_string())]
    );
    assert!(
        layout
            .edges
            .iter()
            .find(|edge| edge.from == "trigger")
            .unwrap()
            .label
            .is_none(),
        "the default port is not worth a label"
    );
}

#[test]
fn forward_follows_an_edge_and_back_returns_along_it() {
    let layout = GraphLayout::build(&diamond());
    let start = layout.index_of("check").unwrap();

    let forward = layout.moved(start, Move::Forward).unwrap();
    assert_eq!(layout.node(forward).unwrap().id, "yes");
    assert_eq!(
        layout
            .node(layout.moved(forward, Move::Back).unwrap())
            .unwrap()
            .id,
        "check"
    );
}

#[test]
fn lane_moves_step_one_lane_at_a_time_and_stop_at_the_edges() {
    let layout = GraphLayout::build(&diamond());
    let top = layout.index_of("yes").unwrap();

    let down = layout.moved(top, Move::LaneDown).unwrap();
    assert_eq!(layout.node(down).unwrap().id, "no");
    assert!(
        layout.moved(down, Move::LaneDown).is_none(),
        "the bottom lane has nowhere below it"
    );
    assert!(layout.moved(top, Move::LaneUp).is_none());
}

#[test]
fn forward_from_a_disconnected_node_still_reaches_the_next_layer() {
    // `orphan` shares layer 0 with the trigger but feeds nothing.
    let mut graph = diamond();
    graph
        .nodes
        .push(node("orphan", NodeKind::Transform, json!({})));

    let layout = GraphLayout::build(&graph);
    let orphan = layout.index_of("orphan").unwrap();

    let next = layout.moved(orphan, Move::Forward).expect("layer 1 exists");
    assert_eq!(layout.node(next).unwrap().layer, 1);
}

#[test]
fn the_last_layer_has_nowhere_forward_to_go() {
    let layout = GraphLayout::build(&diamond());
    let join = layout.index_of("join").unwrap();

    assert!(layout.moved(join, Move::Forward).is_none());
}

#[test]
fn an_empty_graph_lays_out_as_nothing_rather_than_failing() {
    let layout = GraphLayout::build(&WorkflowGraph::default());

    assert_eq!(layout.layers, 0);
    assert_eq!(layout.lanes, 0);
    assert!(layout.moved(0, Move::Forward).is_none());
}

#[test]
fn a_node_with_no_name_is_labelled_with_its_id() {
    let graph = WorkflowGraph {
        nodes: vec![Node {
            name: "   ".to_string(),
            ..node("bare", NodeKind::Agent, json!({}))
        }],
        edges: Vec::new(),
        ..Default::default()
    };

    assert_eq!(GraphLayout::build(&graph).nodes[0].name, "bare");
}

#[test]
fn named_ports_are_carried_and_the_default_one_is_not() {
    let graph = WorkflowGraph {
        nodes: vec![Node {
            ports: vec![
                Port {
                    name: "main".into(),
                    label: None,
                },
                Port {
                    name: "true".into(),
                    label: None,
                },
            ],
            position: Some(Position { x: 1.0, y: 2.0 }),
            ..node("check", NodeKind::Condition, json!({}))
        }],
        edges: Vec::new(),
        ..Default::default()
    };

    assert_eq!(GraphLayout::build(&graph).nodes[0].out_ports, vec!["true"]);
}

#[test]
fn every_kind_has_a_wire_name_a_glyph_and_a_colour() {
    for kind in [
        NodeKind::Trigger,
        NodeKind::Agent,
        NodeKind::ToolCall,
        NodeKind::HttpRequest,
        NodeKind::Code,
        NodeKind::Condition,
        NodeKind::Switch,
        NodeKind::Merge,
        NodeKind::SplitOut,
        NodeKind::Transform,
        NodeKind::OutputParser,
        NodeKind::SubWorkflow,
    ] {
        assert!(!kind_wire(&kind).is_empty(), "{kind:?}");
        assert!(!kind_glyph(&kind).is_empty(), "{kind:?}");
        assert!(!kind_color(&kind).is_empty(), "{kind:?}");
        // The wire name must be the serde one, or a graph's `kind` string and
        // this label would disagree about the same node.
        assert_eq!(
            serde_json::to_value(&kind).unwrap().as_str().unwrap(),
            kind_wire(&kind)
        );
    }
}

#[test]
fn a_summary_names_what_identifies_the_node_for_its_kind() {
    let cases = [
        (
            node(
                "h",
                NodeKind::HttpRequest,
                json!({"method":"post","url":"https://x/y"}),
            ),
            "POST https://x/y",
        ),
        (
            node("a", NodeKind::Agent, json!({"prompt":"first line\nsecond"})),
            "first line",
        ),
        (
            node("c", NodeKind::Condition, json!({"expression":"=.ok"})),
            "=.ok",
        ),
        (
            node("m", NodeKind::Merge, json!({"inputs":["a","b","c"]})),
            "3 inputs",
        ),
        (
            node("t", NodeKind::Trigger, json!({"trigger_kind":"webhook"})),
            "webhook",
        ),
    ];

    for (node, expected) in cases {
        assert_eq!(node_summary(&node), expected, "{}", node.id);
    }
}

#[test]
fn a_config_with_nothing_to_say_summarises_as_empty() {
    assert!(node_summary(&node("a", NodeKind::Agent, json!({}))).is_empty());
    assert!(node_summary(&node("h", NodeKind::HttpRequest, json!({"method":"get"}))).is_empty());
}

#[test]
fn a_blank_config_string_is_not_treated_as_a_summary() {
    assert!(node_summary(&node("a", NodeKind::Agent, json!({"prompt":"  "}))).is_empty());
}
