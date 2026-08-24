//! Unit tests for the decisions a run makes before its first node executes.

use serde_json::json;

use super::super::preflight::clamp_loop_iterations;

/// A one-loop graph whose loop node carries `config` verbatim.
fn loop_graph(config: serde_json::Value) -> tinyflows::model::WorkflowGraph {
    serde_json::from_value(json!({
        "name": "Loop",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "l", "kind": "loop", "name": "Until done", "config": config }
        ],
        "edges": [{ "from_node": "start", "to_node": "l" }]
    }))
    .expect("valid graph")
}

/// The clamped `max_iterations` on node `l`, if it has one.
fn clamped(graph: &tinyflows::model::WorkflowGraph) -> Option<u64> {
    graph
        .nodes
        .iter()
        .find(|node| node.id == "l")
        .and_then(|node| node.config.get("max_iterations"))
        .and_then(serde_json::Value::as_u64)
}

#[test]
fn an_object_config_is_clamped_in_place() {
    let graph = clamp_loop_iterations(loop_graph(json!({ "max_iterations": 50 })), 2);

    assert_eq!(clamped(&graph), Some(2));
}

#[test]
fn a_loop_whose_config_is_not_an_object_is_still_clamped() {
    // A hand-edited document can hold `null` here. Skipping it would leave the
    // node running to the engine's own default while the clamp's log line said
    // otherwise — the ceiling is not advisory.
    for config in [json!(null), json!("nonsense"), json!([1, 2])] {
        let graph = clamp_loop_iterations(loop_graph(config.clone()), 2);

        assert_eq!(clamped(&graph), Some(2), "config {config}");
    }
}

#[test]
fn a_loop_already_inside_the_ceiling_is_left_alone() {
    let graph = clamp_loop_iterations(loop_graph(json!({ "max_iterations": 2 })), 10);

    assert_eq!(clamped(&graph), Some(2));
    // A config that names no cap and is within the ceiling keeps its shape.
    let untouched = clamp_loop_iterations(loop_graph(json!(null)), 10_000);
    assert!(untouched
        .nodes
        .iter()
        .find(|node| node.id == "l")
        .is_some_and(|node| node.config.is_null()));
}
