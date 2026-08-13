//! Tests for the harness gate, and for the composition around it.
//!
//! Two obligations, and the second is the one that keeps a gate trustworthy:
//! it fires on the graph that is guaranteed broken, and it stays silent on
//! everything else. A gate with false positives costs authors their edits and
//! teaches them to route around it.
//!
//! The host-agnostic gates are tested in `tinyflows::gates`, next to the code.
//! What is checked here is the harness gate — which needs this host's list of
//! harnesses — and that [`failures`] still runs the engine's gates as well as
//! this one.

use serde_json::json;
use tinyflows::model::WorkflowGraph;

use super::*;

/// A graph from a node list, with no edges — the gates read configs, not
/// topology, and a trigger would only be noise here.
fn graph(nodes: serde_json::Value) -> WorkflowGraph {
    serde_json::from_value(json!({ "name": "test", "nodes": nodes, "edges": [] }))
        .expect("graph parses")
}

// ---- harness and model selection ----

#[test]
fn a_misspelled_builtin_harness_is_refused() {
    let graph = graph(json!([
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "prompt": "go", "harness": "claude code" } },
    ]));

    let failures = failures(&graph);

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("work"), "{failures:?}");
    assert!(failures[0].contains("custom harness id"), "{failures:?}");
}

#[test]
fn a_harness_written_as_an_expression_is_refused() {
    // The injection case: whichever value flowed down the graph would choose
    // which binary and which credentials run the next instruction.
    let graph = graph(json!([
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "prompt": "go", "harness": "=nodes.pick.item.json.harness" } },
    ]));

    let failures = failures(&graph);

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].contains("decision the graph makes"),
        "{failures:?}"
    );
}

#[test]
fn a_non_string_harness_is_refused_by_type() {
    let graph = graph(json!([
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "prompt": "go", "harness": true } },
    ]));

    let failures = failures(&graph);

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("must be a string"), "{failures:?}");
}

#[test]
fn the_provider_alias_is_checked_too() {
    let graph = graph(json!([
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "prompt": "go", "provider": "claude code" } },
    ]));

    assert_eq!(failures(&graph).len(), 1);
}

#[test]
fn a_legitimate_harness_choice_passes() {
    // Built-in names, custom preset ids, a model on its own, and an omitted
    // choice are all ordinary authoring — a gate that fired on any of them
    // would cost an author their edit for nothing.
    let graph = graph(json!([
        { "id": "a", "kind": "agent", "name": "A",
          "config": { "prompt": "go", "harness": "codex", "model": "gpt-5-codex" } },
        { "id": "b", "kind": "agent", "name": "B",
          "config": { "prompt": "go", "harness": "deepseek-claude" } },
        { "id": "c", "kind": "agent", "name": "C",
          "config": { "prompt": "go", "model": "=nodes.pick.item.json.model" } },
        { "id": "d", "kind": "agent", "name": "D", "config": { "prompt": "go" } },
        { "id": "e", "kind": "agent", "name": "E",
          "config": { "prompt": "go", "harness": "" } },
    ]));

    assert!(failures(&graph).is_empty(), "{:?}", failures(&graph));
}

#[test]
fn a_harness_on_a_node_that_is_not_an_agent_is_not_this_gate_s_business() {
    // Other node kinds do not dispatch to a harness, so the key is inert there
    // — refusing it would be a gate firing on a graph that works.
    let graph = graph(json!([
        { "id": "t", "kind": "transform", "name": "T",
          "config": { "harness": "not a harness" } },
    ]));

    assert!(failures(&graph).is_empty());
}

// ---- composition ----

#[test]
fn the_engines_own_gates_still_run_alongside_the_harness_one() {
    // The trap in overriding `HostPolicy::check_graph`: replacing the default
    // wholesale is easy, and it would silently stop catching everything
    // `tinyflows::gates` catches. One graph, one failure from each side.
    let graph = graph(json!([
        { "id": "fetch", "kind": "agent", "name": "Fetch",
          "config": { "prompt": "=Go and fetch the issues" } },
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "prompt": "go", "harness": "claude code" } },
    ]));

    let failures = failures(&graph);

    assert_eq!(failures.len(), 2, "{failures:?}");
    assert!(
        failures.iter().any(|f| f.contains("does not interpolate")),
        "the engine's prompt gate must still run: {failures:?}"
    );
    assert!(
        failures.iter().any(|f| f.contains("custom harness id")),
        "this host's harness gate must run: {failures:?}"
    );
}

#[test]
fn check_turns_the_combined_list_into_one_error() {
    let graph = graph(json!([
        { "id": "work", "kind": "agent", "name": "Work",
          "config": { "prompt": "go", "harness": "claude code" } },
    ]));

    let err = check("sweep", &graph).expect_err("refused");

    let WorkflowError::Invalid { id, messages } = err else {
        panic!("expected Invalid");
    };
    assert_eq!(id, "sweep");
    assert_eq!(messages.len(), 1, "{messages:?}");
}
