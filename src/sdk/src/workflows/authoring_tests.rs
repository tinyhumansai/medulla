//! Tests for patch-based workflow editing.
//!
//! The theme is that a bad edit costs an error message, never the saved
//! workflow.

use std::sync::Arc;

use serde_json::json;
use tinyflows::graph_ops::GraphOp;

use super::{
    apply_workflow_ops, create_workflow, preview_workflow_ops, validate_handle, GraphHandle,
};
use crate::workflows::{FileWorkflowStore, WorkflowError, WorkflowStore};

fn document(id: &str) -> String {
    json!({
        "id": id,
        "name": "Sweep",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "do it", "agent_ref": "builder" } }
        ],
        "edges": [{ "from_node": "t", "to_node": "work" }]
    })
    .to_string()
}

fn store() -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    (root, store)
}

#[test]
fn a_config_patch_merges_rather_than_replacing_the_whole_config() {
    let (_root, store) = store();
    create_workflow(&store, &document("sweep"), "sweep").unwrap();

    let record = apply_workflow_ops(
        &store,
        "sweep",
        &[GraphOp::UpdateNodeConfig {
            id: "work".into(),
            config: json!({ "prompt": "do it carefully" }),
        }],
    )
    .expect("applies");

    let node = record.graph.node("work").unwrap();
    assert_eq!(node.config["prompt"], "do it carefully");
    assert_eq!(
        node.config["agent_ref"], "builder",
        "a merge patch must not drop the fields it did not mention"
    );
}

#[test]
fn a_null_leaf_in_a_patch_deletes_that_key() {
    let (_root, store) = store();
    create_workflow(&store, &document("sweep"), "sweep").unwrap();

    let record = apply_workflow_ops(
        &store,
        "sweep",
        &[GraphOp::UpdateNodeConfig {
            id: "work".into(),
            config: json!({ "agent_ref": null }),
        }],
    )
    .expect("applies");

    assert!(
        record
            .graph
            .node("work")
            .unwrap()
            .config
            .get("agent_ref")
            .is_none(),
        "the node should fall back to the default worker"
    );
}

#[test]
fn an_op_naming_a_node_that_does_not_exist_leaves_the_workflow_untouched() {
    let (_root, store) = store();
    create_workflow(&store, &document("sweep"), "sweep").unwrap();

    let err = apply_workflow_ops(
        &store,
        "sweep",
        &[GraphOp::SetNodeName {
            id: "ghost".into(),
            name: "nope".into(),
        }],
    )
    .expect_err("no such node");

    assert!(err.to_string().contains("ghost"), "name the node: {err}");
    assert_eq!(
        store
            .get("sweep")
            .unwrap()
            .unwrap()
            .graph
            .node("work")
            .unwrap()
            .name,
        "Work",
        "a failed edit must not have been half-applied"
    );
}

#[test]
fn an_edit_that_would_break_the_graph_is_refused_before_it_is_saved() {
    let (_root, store) = store();
    create_workflow(&store, &document("sweep"), "sweep").unwrap();

    // Removing the trigger leaves a graph the engine will not compile.
    let err = apply_workflow_ops(&store, "sweep", &[GraphOp::RemoveNode { id: "t".into() }])
        .expect_err("must be refused");

    assert!(matches!(err, WorkflowError::Invalid { .. }), "got {err:?}");
    assert!(
        store
            .get("sweep")
            .unwrap()
            .unwrap()
            .graph
            .node("t")
            .is_some(),
        "the saved workflow must still have its trigger"
    );
}

#[test]
fn a_batch_of_ops_reports_which_one_failed() {
    let (_root, store) = store();
    create_workflow(&store, &document("sweep"), "sweep").unwrap();

    let err = apply_workflow_ops(
        &store,
        "sweep",
        &[
            GraphOp::SetNodeName {
                id: "work".into(),
                name: "Renamed".into(),
            },
            GraphOp::SetNodeName {
                id: "ghost".into(),
                name: "nope".into(),
            },
        ],
    )
    .expect_err("the second op fails");

    assert!(
        err.to_string().contains("ghost"),
        "the message should identify the failing op: {err}"
    );
    assert_eq!(
        store
            .get("sweep")
            .unwrap()
            .unwrap()
            .graph
            .node("work")
            .unwrap()
            .name,
        "Work",
        "the first op must not survive the batch failing"
    );
}

#[test]
fn a_preview_checks_the_edit_without_writing_it() {
    let (_root, store) = store();
    create_workflow(&store, &document("sweep"), "sweep").unwrap();

    let previewed = preview_workflow_ops(
        &store,
        "sweep",
        &[GraphOp::SetNodeName {
            id: "work".into(),
            name: "Renamed".into(),
        }],
    )
    .expect("previews");

    assert_eq!(previewed.node("work").unwrap().name, "Renamed");
    assert_eq!(
        store
            .get("sweep")
            .unwrap()
            .unwrap()
            .graph
            .node("work")
            .unwrap()
            .name,
        "Work",
        "a preview must not save"
    );
}

#[test]
fn an_inline_graph_can_be_validated_without_saving_it_first() {
    let (_root, store) = store();

    let record =
        validate_handle(&store, &GraphHandle::Inline(&document("draft"))).expect("a valid draft");

    assert_eq!(record.id, "draft");
    assert!(
        store.list().unwrap().is_empty(),
        "validating a draft must not install it"
    );
}

#[test]
fn validating_an_inline_graph_reports_every_problem_at_once() {
    let (_root, store) = store();
    let broken = json!({
        "id": "broken",
        "nodes": [{ "id": "a", "kind": "transform", "name": "a" }],
        "edges": [{ "from_node": "a", "to_node": "ghost" }]
    })
    .to_string();

    let err = validate_handle(&store, &GraphHandle::Inline(&broken)).expect_err("invalid");

    let WorkflowError::Invalid { messages, .. } = err else {
        panic!("expected Invalid");
    };
    assert!(
        messages.len() >= 2,
        "one round-trip should tell an author everything: {messages:?}"
    );
}

#[test]
fn a_saved_handle_and_an_inline_handle_resolve_the_same_way() {
    let (_root, store) = store();
    create_workflow(&store, &document("sweep"), "sweep").unwrap();

    let saved = GraphHandle::Saved("sweep").resolve(&store).unwrap();
    let inline = GraphHandle::Inline(&document("sweep"))
        .resolve(&store)
        .unwrap();

    assert_eq!(saved.graph, inline.graph);
}

#[test]
fn editing_a_workflow_that_does_not_exist_says_so() {
    let (_root, store) = store();

    let err = apply_workflow_ops(&store, "ghost", &[]).expect_err("no such workflow");

    assert!(matches!(err, WorkflowError::NotFound(_)), "got {err:?}");
}
