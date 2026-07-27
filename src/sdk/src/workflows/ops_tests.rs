//! Tests for the shared operation surface.
//!
//! These are the operations a harness calls over a tool boundary, so the tests
//! care as much about the *shape* of what comes back as about the effect — a
//! model can only act on what it can read.

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::workflows::FileWorkflowStore;

fn document(id: &str) -> String {
    json!({
        "id": id,
        "name": "Sweep",
        "description": "sweeps",
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
fn create_then_list_then_get_round_trips_a_workflow() {
    let (_root, store) = store();

    create(&store, &document("sweep"), "sweep").expect("creates");

    let listed = list(&store).unwrap();
    assert_eq!(listed["workflows"][0]["id"], "sweep");
    assert_eq!(listed["workflows"][0]["nodeCount"], 2);
    assert_eq!(listed["workflows"][0]["triggerKind"], "manual");

    let fetched = get(&store, "sweep").unwrap();
    assert_eq!(fetched["description"], "sweeps");
    assert_eq!(fetched["graph"]["nodes"].as_array().unwrap().len(), 2);
}

#[test]
fn ops_are_accepted_as_a_bare_array_or_wrapped_in_an_object() {
    let (_root, store) = store();
    create(&store, &document("sweep"), "sweep").unwrap();

    let bare = json!([{ "op": "set_node_name", "id": "work", "name": "Renamed" }]);
    apply_ops(&store, "sweep", &bare).expect("bare array");

    let wrapped = json!({ "ops": [{ "op": "set_node_name", "id": "work", "name": "Again" }] });
    let result = apply_ops(&store, "sweep", &wrapped).expect("wrapped object");

    assert_eq!(result["workflow"]["graph"]["nodes"][1]["name"], "Again");
}

#[test]
fn a_malformed_ops_payload_says_what_was_expected() {
    let (_root, store) = store();
    create(&store, &document("sweep"), "sweep").unwrap();

    let err = apply_ops(&store, "sweep", &json!("nonsense")).expect_err("not ops");
    assert!(err.to_string().contains("array of ops"), "got {err}");

    let unknown =
        apply_ops(&store, "sweep", &json!([{ "op": "teleport" }])).expect_err("no such op");
    assert!(unknown.to_string().contains("invalid ops"), "got {unknown}");
}

#[test]
fn a_preview_reports_ok_without_saving() {
    let (_root, store) = store();
    create(&store, &document("sweep"), "sweep").unwrap();

    let preview = preview_ops(
        &store,
        "sweep",
        &json!([{ "op": "set_node_name", "id": "work", "name": "Renamed" }]),
    )
    .unwrap();

    assert_eq!(preview["ok"], true);
    assert_eq!(
        get(&store, "sweep").unwrap()["graph"]["nodes"][1]["name"],
        "Work"
    );
}

#[test]
fn validation_answers_rather_than_failing_when_a_graph_is_invalid() {
    // An author asking "is this valid" has been answered either way; making
    // them handle an error to read the answer would be perverse.
    let (_root, store) = store();
    let broken = json!({
        "id": "broken",
        "nodes": [{ "id": "a", "kind": "transform", "name": "a" }],
        "edges": []
    })
    .to_string();

    let result = validate(&store, &GraphHandle::Inline(&broken));

    assert_eq!(result["ok"], false);
    assert!(
        result["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.as_str().unwrap().contains("missing_trigger")),
        "got {result}"
    );
}

#[test]
fn validating_a_good_graph_reports_its_size() {
    let (_root, store) = store();
    create(&store, &document("sweep"), "sweep").unwrap();

    let result = validate(&store, &GraphHandle::Saved("sweep"));

    assert_eq!(result["ok"], true);
    assert_eq!(result["nodes"], 2);
    assert_eq!(result["edges"], 1);
}

#[tokio::test]
async fn a_dry_run_returns_the_output_of_every_node() {
    let (_root, store) = store();
    create(&store, &document("sweep"), "sweep").unwrap();

    let result = dry_run(&store, "sweep", json!({}))
        .await
        .expect("simulates");

    assert_eq!(result["ok"], true);
    assert!(
        result["output"]["nodes"]["work"].is_object(),
        "the agent node should have produced output: {result}"
    );
}

#[test]
fn the_catalog_lists_every_kind_and_can_be_narrowed_to_one() {
    let all = catalog(None).unwrap();
    assert_eq!(
        all["contracts"].as_array().unwrap().len(),
        crate::workflows::node_contracts::NODE_KINDS.len()
    );

    let one = catalog(Some("agent")).unwrap();
    assert_eq!(one["contract"]["kind"], "agent");
    assert!(
        one["contract"]["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("harness")),
        "the host overlay should be present: {one}"
    );
}

#[test]
fn asking_for_an_unknown_node_kind_lists_the_ones_that_exist() {
    let err = catalog(Some("teleport")).expect_err("no such kind");

    assert!(err.to_string().contains("agent"), "got {err}");
}

#[test]
fn deleting_a_workflow_removes_it_from_the_listing() {
    let (_root, store) = store();
    create(&store, &document("sweep"), "sweep").unwrap();

    delete(&store, "sweep").expect("deletes");

    assert!(list(&store).unwrap()["workflows"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn run_history_is_empty_rather_than_missing_for_a_workflow_that_never_ran() {
    let (_root, store) = store();
    create(&store, &document("sweep"), "sweep").unwrap();

    let runs = list_runs(&store, "sweep").unwrap();

    assert_eq!(runs["runs"].as_array().unwrap().len(), 0);
}

#[test]
fn cancelling_a_run_that_is_not_executing_reports_that_plainly() {
    let result = cancel_run("never-started");

    assert_eq!(result["cancelled"], false);
    assert_eq!(result["runId"], "never-started");
}
