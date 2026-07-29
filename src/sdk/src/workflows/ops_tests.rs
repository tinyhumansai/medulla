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

#[test]
fn history_carries_the_whole_graph_of_each_earlier_version() {
    let (_root, store) = store();
    create(&store, &document("sweep"), "sweep").unwrap();
    apply_ops(
        &store,
        "sweep",
        &json!([{ "op": "set_node_name", "id": "work", "name": "Renamed" }]),
    )
    .unwrap();

    let history = list_history(&store, "sweep").unwrap();

    let revisions = history["revisions"].as_array().expect("array");
    assert_eq!(revisions.len(), 1);
    // The graph is included: history is read to decide which version to go back
    // to, and an id with a timestamp does not support that decision.
    assert_eq!(
        revisions[0]["record"]["graph"]["nodes"][1]["name"],
        json!("Work")
    );
}

#[test]
fn history_for_a_workflow_that_does_not_exist_is_an_error_not_an_empty_list() {
    let (_root, store) = store();

    let err = list_history(&store, "absent").expect_err("must refuse");

    // An empty history would read as "never edited" for something that is not
    // there at all.
    assert!(matches!(err, WorkflowError::NotFound(_)), "got {err:?}");
}

#[test]
fn undo_puts_the_previous_graph_back_and_says_what_it_restored() {
    let (_root, store) = store();
    create(&store, &document("sweep"), "sweep").unwrap();
    apply_ops(
        &store,
        "sweep",
        &json!([{ "op": "set_node_name", "id": "work", "name": "Renamed" }]),
    )
    .unwrap();

    let undone = undo(&store, "sweep").unwrap();

    assert_eq!(undone["undone"], json!(true));
    assert_eq!(
        undone["workflow"]["graph"]["nodes"][1]["name"],
        json!("Work")
    );
    assert!(undone["revision"].is_string());
    assert!(undone["supersededAt"].as_u64().unwrap() > 0);
}

#[test]
fn undo_on_a_workflow_that_was_never_edited_explains_itself_rather_than_failing() {
    let (_root, store) = store();
    create(&store, &document("sweep"), "sweep").unwrap();

    let undone = undo(&store, "sweep").unwrap();

    assert_eq!(undone["undone"], json!(false));
    assert!(
        undone["reason"]
            .as_str()
            .unwrap()
            .contains("nothing to go back to"),
        "{undone}"
    );
}

#[test]
fn rolling_back_names_the_revision_it_used() {
    let (_root, store) = store();
    create(&store, &document("sweep"), "sweep").unwrap();
    apply_ops(
        &store,
        "sweep",
        &json!([{ "op": "set_node_name", "id": "work", "name": "Renamed" }]),
    )
    .unwrap();
    let history = list_history(&store, "sweep").unwrap();
    let revision = history["revisions"][0]["id"].as_str().unwrap().to_string();

    let restored = rollback(&store, "sweep", &revision).unwrap();

    assert_eq!(restored["restored"], json!("sweep"));
    assert_eq!(restored["revision"], json!(revision));
    assert_eq!(
        restored["workflow"]["graph"]["nodes"][1]["name"],
        json!("Work")
    );
}

#[tokio::test]
async fn a_dry_run_reports_a_failure_the_error_policy_swallowed() {
    let (_root, store) = store();
    // The blind spot these diagnostics exist for. `medulla:echo` refuses a null
    // argument, so this node fails — but `on_error: continue` means the *run*
    // succeeds, the outcome is a JSON object, and the failed step carries no
    // diagnostics of its own. Everything downstream reads it as a clean run.
    create(
        &store,
        &json!({
            "id": "quiet",
            "name": "Quiet",
            "nodes": [
                { "id": "t", "kind": "trigger", "name": "start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "say", "kind": "tool_call", "name": "Say",
                  "config": { "slug": "medulla:echo", "on_error": "continue",
                              "args": { "text": "=run.trigger.absent" } } }
            ],
            "edges": [{ "from_node": "t", "to_node": "say" }]
        })
        .to_string(),
        "quiet",
    )
    .unwrap();

    let result = dry_run(&store, "quiet", json!({})).await.unwrap();

    // The single most misleading thing this surface could say is `ok: true` for
    // a graph whose work did not happen.
    assert_eq!(result["ok"], json!(false), "{result}");
    let hidden = result["diagnostics"]["hiddenErrors"]
        .as_array()
        .expect("array");
    assert_eq!(hidden.len(), 1, "{result}");
    assert_eq!(hidden[0]["nodeId"], json!("say"));
}

#[tokio::test]
async fn a_dry_run_of_a_soundly_wired_graph_reports_ok() {
    let (_root, store) = store();
    create(
        &store,
        &json!({
            "id": "loud",
            "name": "Loud",
            "nodes": [
                { "id": "t", "kind": "trigger", "name": "start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "say", "kind": "tool_call", "name": "Say",
                  "config": { "slug": "medulla:echo", "args": { "text": "hello" } } }
            ],
            "edges": [{ "from_node": "t", "to_node": "say" }]
        })
        .to_string(),
        "loud",
    )
    .unwrap();

    let result = dry_run(&store, "loud", json!({})).await.unwrap();

    assert_eq!(result["ok"], json!(true), "{result}");
}

#[test]
fn an_edit_whose_binding_would_resolve_null_is_refused_before_it_is_saved() {
    let (_root, store) = store();
    create(&store, &document("sweep"), "sweep").unwrap();

    // `work` is an agent node, so its output is wrapped as {json, text, raw} —
    // reading `.item.title` off it resolves null at run time.
    let err = apply_ops(
        &store,
        "sweep",
        &json!([
            { "op": "add_node", "node": { "id": "say", "kind": "tool_call", "name": "Say",
              "config": { "slug": "medulla:echo",
                          "args": { "text": "=nodes.work.item.title" } } } },
            { "op": "add_edge", "edge": { "from_node": "work", "to_node": "say" } }
        ]),
    )
    .expect_err("must refuse");

    let WorkflowError::Invalid { messages, .. } = err else {
        panic!("expected Invalid, got {err:?}");
    };
    assert!(messages[0].contains("json"), "{messages:?}");
    // Refused whole: the workflow is left exactly as it was.
    let after = get(&store, "sweep").unwrap();
    assert_eq!(after["graph"]["nodes"].as_array().unwrap().len(), 2);
}
