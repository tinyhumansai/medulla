//! Unit tests for the workflow record's derived views and for resolving
//! sub-workflows out of a store.

use std::sync::Arc;

use serde_json::json;
use tinyflows::caps::WorkflowResolver;

use super::store::{parse_workflow, FileWorkflowStore, WorkflowStore};
use super::{RunStatus, StoreWorkflowResolver};

fn document(id: &str, trigger_kind: &str) -> String {
    json!({
        "id": id,
        "name": "Nightly sweep",
        "description": "runs the sweep",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": trigger_kind } },
            { "id": "work", "kind": "transform", "name": "work",
              "config": { "set": { "ok": true } } }
        ],
        "edges": [{ "from_node": "t", "to_node": "work" }]
    })
    .to_string()
}

#[test]
fn a_summary_carries_what_a_listing_needs_without_the_graph() {
    let record = parse_workflow(&document("sweep", "schedule"), "sweep").unwrap();

    let summary = record.summary();

    assert_eq!(summary.id, "sweep");
    assert_eq!(summary.name, "Nightly sweep");
    assert_eq!(summary.description, "runs the sweep");
    assert_eq!(summary.node_count, 2);
    assert_eq!(summary.trigger_kind.as_deref(), Some("schedule"));
    assert!(summary.enabled);
}

#[test]
fn a_graph_whose_trigger_omits_its_kind_reports_none_rather_than_guessing() {
    let document = json!({
        "id": "bare",
        "nodes": [{ "id": "t", "kind": "trigger", "name": "start" }],
        "edges": []
    })
    .to_string();

    let record = parse_workflow(&document, "bare").unwrap();

    assert_eq!(record.trigger_kind(), None);
}

#[test]
fn only_running_and_pending_approval_are_unsettled() {
    assert!(!RunStatus::Running.is_settled());
    assert!(!RunStatus::PendingApproval.is_settled());
    for status in [
        RunStatus::Succeeded,
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Interrupted,
    ] {
        assert!(status.is_settled(), "{status:?} should be terminal");
    }
}

#[tokio::test]
async fn the_resolver_hands_the_engine_a_saved_sub_workflow() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    let record = parse_workflow(&document("child", "execute_by_workflow"), "child").unwrap();
    store.save(&record).unwrap();

    let resolver = StoreWorkflowResolver::new(store, u64::MAX);
    let graph = resolver.resolve("child").await.expect("resolves");

    assert_eq!(graph.nodes.len(), 2);
}

#[tokio::test]
async fn resolving_an_unknown_sub_workflow_names_the_id_it_could_not_find() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));

    let resolver = StoreWorkflowResolver::new(store, u64::MAX);
    let err = resolver
        .resolve("ghost")
        .await
        .expect_err("no such workflow");

    assert!(
        err.to_string().contains("ghost"),
        "the message should name the id: {err}"
    );
}
