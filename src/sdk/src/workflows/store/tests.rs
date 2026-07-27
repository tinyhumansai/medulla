//! Unit tests for workflow directory layering, document parsing, and the
//! file-backed store's read/write/delete and run-history behaviour.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::file::{new_run_record, parse_workflow, validate_graph, workflow_dirs};
use super::{require, require_run, FileWorkflowStore, WorkflowStore};
use crate::workflows::types::{RunStatus, WorkflowError, WorkflowRecord};

/// A store rooted in a temporary directory, with definitions and runs kept
/// apart the way the discovered layout keeps them.
fn store_in(root: &Path) -> FileWorkflowStore {
    FileWorkflowStore::new(vec![root.join("workflows")], root.join("runs"))
}

/// The smallest document that validates: one trigger, one transform, one edge.
fn valid_document(id: &str) -> String {
    json!({
        "id": id,
        "name": "Greet",
        "description": "says hello",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "greet", "kind": "transform", "name": "greet",
              "config": { "set": { "greeting": "=.item.name" } } }
        ],
        "edges": [
            { "from_node": "t", "from_port": "main", "to_node": "greet", "to_port": "main" }
        ]
    })
    .to_string()
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

#[test]
fn the_directories_are_home_then_project_lowest_precedence_first() {
    let env = HashMap::from([("MEDULLA_HOME".to_string(), "/somewhere/home".to_string())]);
    let dirs = workflow_dirs(&env, Path::new("/repo"));

    assert_eq!(
        dirs,
        vec![
            PathBuf::from("/somewhere/home/workflows"),
            PathBuf::from("/repo/.medulla/workflows"),
        ]
    );
}

#[test]
fn a_dev_home_that_is_the_project_directory_is_listed_once() {
    // Under MEDULLA_DEV the home *is* ./.medulla, so the two candidates name one
    // directory that no filesystem call will confirm — reading it twice would
    // make every workflow shadow itself.
    let env = HashMap::from([("MEDULLA_DEV".to_string(), "1".to_string())]);
    let dirs = workflow_dirs(&env, Path::new("."));

    assert_eq!(dirs.len(), 1, "expected one directory, got {dirs:?}");
}

#[test]
fn parsing_names_a_workflow_by_its_filename_when_the_document_omits_an_id() {
    let document = json!({
        "nodes": [{ "id": "t", "kind": "trigger", "name": "start" }],
        "edges": []
    })
    .to_string();

    let record = parse_workflow(&document, "nightly-sweep").expect("parses");

    assert_eq!(record.id, "nightly-sweep");
    // Name falls back to the id so a listing is never blank.
    assert_eq!(record.name, "nightly-sweep");
    assert!(record.enabled, "workflows are enabled unless opted out");
}

#[test]
fn parsing_migrates_a_document_saved_without_a_schema_version() {
    // A document predating the field must keep loading; the engine's migration
    // runs before deserialization, not after.
    let document = json!({
        "id": "old",
        "nodes": [{ "id": "t", "kind": "trigger", "name": "start" }],
        "edges": []
    })
    .to_string();

    let record = parse_workflow(&document, "old").expect("parses");

    assert_eq!(
        record.graph.schema_version,
        tinyflows::model::CURRENT_SCHEMA_VERSION
    );
}

#[test]
fn parsing_rejects_a_document_that_is_not_an_object() {
    let err = parse_workflow("[]", "list").expect_err("an array is not a workflow");
    assert!(err.contains("object"), "unhelpful message: {err}");
}

#[test]
fn validation_reports_every_failure_not_only_the_first() {
    // A graph with no trigger *and* an edge to a node that does not exist. An
    // author — often an agent editing over a tool call — should learn both in
    // one round-trip.
    let graph = serde_json::from_value(json!({
        "nodes": [{ "id": "a", "kind": "transform", "name": "a" }],
        "edges": [{ "from_node": "a", "to_node": "ghost" }]
    }))
    .unwrap();

    let err = validate_graph("broken", &graph).expect_err("invalid");
    let WorkflowError::Invalid { messages, .. } = err else {
        panic!("expected Invalid, got {err:?}");
    };

    assert!(
        messages.len() >= 2,
        "expected every failure, got {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("missing_trigger")),
        "missing trigger not reported: {messages:?}"
    );
}

#[test]
fn a_project_directory_overrides_a_home_workflow_of_the_same_id_in_place() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let project = root.path().join("project");

    write(&home.join("first.json"), &valid_document("first"));
    write(&home.join("shared.json"), &valid_document("shared"));
    let overridden = valid_document("shared").replace("\"Greet\"", "\"Project greet\"");
    write(&project.join("shared.json"), &overridden);

    let store = FileWorkflowStore::new(vec![home, project], root.path().join("runs"));
    let report = store.load();

    assert!(report.errors.is_empty(), "unexpected: {:?}", report.errors);
    let ids: Vec<&str> = report.workflows.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["first", "shared"],
        "an override should keep the position of what it overrides"
    );
    assert_eq!(report.workflows[1].name, "Project greet");
}

#[test]
fn one_malformed_document_costs_only_itself() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("workflows");
    write(&dir.join("good.json"), &valid_document("good"));
    write(&dir.join("broken.json"), "{ not json");

    let store = store_in(root.path());
    let report = store.load();

    assert_eq!(
        report.workflows.len(),
        1,
        "the good document should survive"
    );
    assert_eq!(report.errors.len(), 1);
    assert!(
        report.errors[0].contains("broken.json"),
        "the error should name the file: {:?}",
        report.errors
    );
}

#[test]
fn a_missing_directory_is_not_an_error() {
    let root = tempfile::tempdir().unwrap();
    let report = store_in(root.path()).load();

    assert!(report.workflows.is_empty());
    assert!(report.errors.is_empty(), "unexpected: {:?}", report.errors);
    assert!(report.dirs.is_empty(), "nothing was read");
}

#[test]
fn saving_then_loading_round_trips_the_host_fields_and_the_graph() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let mut record = parse_workflow(&valid_document("round"), "round").unwrap();
    record.description = "edited".into();
    record.enabled = false;

    store.save(&record).expect("saves");
    let loaded = require(&store, "round").expect("found");

    assert_eq!(loaded.description, "edited");
    assert!(!loaded.enabled, "enabled must survive the round trip");
    assert_eq!(loaded.graph, record.graph);
    assert_eq!(loaded.trigger_kind().as_deref(), Some("manual"));
}

#[test]
fn saving_refuses_a_graph_the_engine_would_not_compile() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let record = WorkflowRecord {
        id: "no-trigger".into(),
        name: "no trigger".into(),
        description: String::new(),
        enabled: true,
        graph: serde_json::from_value(json!({ "nodes": [], "edges": [] })).unwrap(),
        source_path: None,
    };

    let err = store.save(&record).expect_err("must not persist");

    assert!(matches!(err, WorkflowError::Invalid { .. }), "got {err:?}");
    assert!(
        store.list().unwrap().is_empty(),
        "an invalid graph must not reach the catalog"
    );
}

#[test]
fn deleting_removes_the_file_the_workflow_was_actually_read_from() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let record = parse_workflow(&valid_document("gone"), "gone").unwrap();
    store.save(&record).unwrap();

    store.delete("gone").expect("deletes");

    assert!(store.get("gone").unwrap().is_none());
    let err = store
        .delete("gone")
        .expect_err("deleting twice is an error");
    assert!(matches!(err, WorkflowError::NotFound(_)), "got {err:?}");
}

#[test]
fn an_id_containing_a_dot_does_not_collide_on_its_temporary_file() {
    // The temp name is appended, not substituted for the extension, so
    // `a.b.json` and `a.json` cannot fight over one scratch path.
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    store
        .save(&parse_workflow(&valid_document("a.b"), "a.b").unwrap())
        .unwrap();
    store
        .save(&parse_workflow(&valid_document("a"), "a").unwrap())
        .unwrap();

    // Load order is the sorted filename order, so `a.b.json` precedes `a.json`.
    let ids: Vec<String> = store.list().unwrap().into_iter().map(|s| s.id).collect();
    assert_eq!(ids, vec!["a.b", "a"], "both should survive");
}

#[test]
fn runs_are_listed_newest_first_and_scoped_to_their_workflow() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());

    store
        .record_run(&new_run_record("r1", "alpha", 100))
        .unwrap();
    store
        .record_run(&new_run_record("r2", "alpha", 300))
        .unwrap();
    store
        .record_run(&new_run_record("r3", "beta", 200))
        .unwrap();

    let alpha = store.list_runs("alpha").unwrap();
    let ids: Vec<&str> = alpha.iter().map(|r| r.id.as_str()).collect();

    assert_eq!(ids, vec!["r2", "r1"], "newest first");
    assert_eq!(store.list_runs("beta").unwrap().len(), 1);
    assert_eq!(store.list_runs("unknown").unwrap().len(), 0);
}

#[test]
fn a_run_record_survives_being_rewritten_as_it_settles() {
    let root = tempfile::tempdir().unwrap();
    let store = store_in(root.path());
    let mut run = new_run_record("r1", "alpha", 100);
    store.record_run(&run).unwrap();

    run.status = RunStatus::PendingApproval;
    run.pending_approvals = vec!["review".into()];
    store.record_run(&run).unwrap();

    let loaded = require_run(&store, "r1").expect("found");
    assert_eq!(loaded.status, RunStatus::PendingApproval);
    assert_eq!(loaded.pending_approvals, vec!["review".to_string()]);
    assert!(!loaded.status.is_settled(), "an approval gate is resumable");
}

#[test]
fn asking_for_a_run_that_was_never_recorded_is_an_error_not_a_silent_none() {
    let root = tempfile::tempdir().unwrap();
    let err = require_run(&store_in(root.path()), "ghost").expect_err("no such run");
    assert!(matches!(err, WorkflowError::RunNotFound(_)), "got {err:?}");
}
