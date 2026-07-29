//! Tests for the before/after graph difference.

use super::*;
use serde_json::json;
use tinyflows::model::{Edge, NodeKind, Port};

fn node(id: &str, kind: NodeKind) -> Node {
    Node {
        id: id.to_string(),
        kind,
        type_version: 1,
        name: id.to_string(),
        config: json!({}),
        ports: Vec::new(),
        position: None,
    }
}

fn edge(from: &str, to: &str) -> Edge {
    Edge {
        from_node: from.to_string(),
        from_port: "main".to_string(),
        to_node: to.to_string(),
        to_port: "main".to_string(),
    }
}

/// A record around a bare graph.
///
/// The diff reads whole records, so even the purely structural cases go through
/// one. Everything outside the graph is held constant here so those cases keep
/// asserting only the structure.
fn graph(nodes: Vec<Node>, edges: Vec<Edge>) -> WorkflowRecord {
    record(WorkflowGraph {
        nodes,
        edges,
        ..Default::default()
    })
}

/// A record wrapping `graph`, named after it as the store's parser would.
fn record(graph: WorkflowGraph) -> WorkflowRecord {
    WorkflowRecord {
        id: "wf".into(),
        name: graph.name.clone(),
        description: String::new(),
        enabled: true,
        graph,
        source_path: None,
    }
}

#[test]
fn an_unchanged_graph_reports_nothing() {
    let before = graph(vec![node("a", NodeKind::Trigger)], vec![]);

    assert!(describe(&before, &before.clone()).is_empty());
}

#[test]
fn an_added_node_is_named_with_its_kind() {
    let before = graph(vec![node("a", NodeKind::Trigger)], vec![]);
    let after = graph(
        vec![node("a", NodeKind::Trigger), node("b", NodeKind::ToolCall)],
        vec![],
    );

    assert_eq!(describe(&before, &after), vec!["+ node b (tool_call)"]);
}

#[test]
fn a_removed_node_is_named_with_its_kind() {
    let before = graph(
        vec![node("a", NodeKind::Trigger), node("b", NodeKind::Agent)],
        vec![],
    );
    let after = graph(vec![node("a", NodeKind::Trigger)], vec![]);

    assert_eq!(describe(&before, &after), vec!["− node b (agent)"]);
}

#[test]
fn a_renamed_id_reads_as_a_removal_and_an_addition() {
    let before = graph(vec![node("old", NodeKind::Agent)], vec![]);
    let after = graph(vec![node("new", NodeKind::Agent)], vec![]);

    let changes = describe(&before, &after);

    assert!(
        changes.contains(&"+ node new (agent)".to_string()),
        "{changes:?}"
    );
    assert!(
        changes.contains(&"− node old (agent)".to_string()),
        "{changes:?}"
    );
}

#[test]
fn an_edited_config_is_reported_without_spelling_the_whole_document_out() {
    let before = graph(vec![node("a", NodeKind::Agent)], vec![]);
    let mut edited = node("a", NodeKind::Agent);
    edited.config = json!({"prompt": "a long instruction that would not fit on a transcript line"});
    let after = graph(vec![edited], vec![]);

    assert_eq!(describe(&before, &after), vec!["~ node a config"]);
}

#[test]
fn a_changed_kind_names_both_kinds() {
    let before = graph(vec![node("a", NodeKind::Agent)], vec![]);
    let after = graph(vec![node("a", NodeKind::ToolCall)], vec![]);

    assert_eq!(
        describe(&before, &after),
        vec!["~ node a kind agent → tool_call"]
    );
}

#[test]
fn a_renamed_node_says_its_new_name() {
    let before = graph(vec![node("a", NodeKind::Agent)], vec![]);
    let mut renamed = node("a", NodeKind::Agent);
    renamed.name = "Summarise".into();
    let after = graph(vec![renamed], vec![]);

    assert_eq!(
        describe(&before, &after),
        vec!["~ node a named \"Summarise\""]
    );
}

#[test]
fn changed_ports_are_reported_because_edges_depend_on_them() {
    let before = graph(vec![node("a", NodeKind::Condition)], vec![]);
    let mut ported = node("a", NodeKind::Condition);
    ported.ports = vec![Port {
        name: "true".into(),
        label: None,
    }];
    let after = graph(vec![ported], vec![]);

    assert_eq!(describe(&before, &after), vec!["~ node a ports"]);
}

#[test]
fn added_and_removed_edges_are_both_reported() {
    let nodes = || {
        vec![
            node("a", NodeKind::Trigger),
            node("b", NodeKind::Agent),
            node("c", NodeKind::Agent),
        ]
    };
    let before = graph(nodes(), vec![edge("a", "b")]);
    let after = graph(nodes(), vec![edge("a", "c")]);

    let changes = describe(&before, &after);

    assert!(changes.contains(&"+ edge a → c".to_string()), "{changes:?}");
    assert!(changes.contains(&"− edge a → b".to_string()), "{changes:?}");
}

#[test]
fn an_edge_off_a_named_port_says_which_port() {
    let nodes = || {
        vec![
            node("check", NodeKind::Condition),
            node("go", NodeKind::Agent),
        ]
    };
    let before = graph(nodes(), vec![]);
    let after = graph(
        nodes(),
        vec![Edge {
            from_port: "true".into(),
            ..edge("check", "go")
        }],
    );

    assert_eq!(describe(&before, &after), vec!["+ edge check:true → go"]);
}

#[test]
fn a_renamed_workflow_is_reported() {
    let before = graph(vec![], vec![]);
    let after = record(WorkflowGraph {
        name: "Nightly sweep".into(),
        ..Default::default()
    });

    assert_eq!(describe(&before, &after), vec!["~ name  → Nightly sweep"]);
}

#[test]
fn an_edited_description_is_reported_even_though_the_graph_is_untouched() {
    let before = graph(vec![node("a", NodeKind::Trigger)], vec![]);
    let after = WorkflowRecord {
        description: "sweeps the repo every night".into(),
        ..before.clone()
    };

    // The description is not in the graph, so a graph-only diff saw nothing and
    // the caller skipped its catalogue refresh — leaving a stale rail.
    assert_eq!(describe(&before, &after), vec!["~ description"]);
}

#[test]
fn enabling_and_disabling_each_say_which_way_it_went() {
    let enabled = graph(vec![node("a", NodeKind::Trigger)], vec![]);
    let disabled = WorkflowRecord {
        enabled: false,
        ..enabled.clone()
    };

    assert_eq!(describe(&enabled, &disabled), vec!["~ disabled"]);
    assert_eq!(describe(&disabled, &enabled), vec!["~ enabled"]);
}
