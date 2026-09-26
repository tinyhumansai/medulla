//! Tests for patch-based editing that need Medulla's own operations.
//!
//! The rest of the authoring suite moved to `tinyflows::store::authoring` with
//! the code. What stays is the case that crosses into this crate: a graph edit
//! and a `defaults` edit racing through two independent store instances, where
//! the second writer is `workflows::ops::set_defaults_observed` and so cannot be
//! expressed from inside the engine crate.

use std::sync::Arc;

use serde_json::json;
use tinyflows::graph_ops::GraphOp;
use tinyflows::store::{apply_workflow_ops_observed, create_workflow};

use crate::workflows::{FileWorkflowStore, WorkflowStore};

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

#[test]
fn graph_ops_and_defaults_rebase_without_reverting_each_other() {
    let root = tempfile::tempdir().unwrap();
    let definitions = root.path().join("workflows");
    let runs = root.path().join("runs");
    let graph_store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![definitions.clone()],
        runs.clone(),
    ));
    let defaults_store: Arc<dyn WorkflowStore> =
        Arc::new(FileWorkflowStore::new(vec![definitions.clone()], runs));
    create_workflow(&graph_store, &document("sweep"), "sweep").unwrap();

    let barrier = Arc::new(std::sync::Barrier::new(2));

    let graph_barrier = barrier.clone();
    let graph_edit = std::thread::spawn(move || {
        apply_workflow_ops_observed(
            &graph_store,
            "sweep",
            &[GraphOp::SetNodeName {
                id: "work".into(),
                name: "Renamed".into(),
            }],
            |attempt| {
                if attempt == 1 {
                    graph_barrier.wait();
                }
            },
        )
    });
    let defaults_barrier = barrier.clone();
    let defaults_edit = std::thread::spawn(move || {
        crate::workflows::ops::set_defaults_observed(
            &defaults_store,
            "sweep",
            Some("codex"),
            Some("gpt-5"),
            |attempt| {
                if attempt == 1 {
                    defaults_barrier.wait();
                }
            },
        )
    });
    let (_, graph_attempts) = graph_edit.join().unwrap().unwrap();
    let (_, defaults_attempts) = defaults_edit.join().unwrap().unwrap();
    assert!(
        graph_attempts > 1 || defaults_attempts > 1,
        "one stale whole-record CAS must rebase"
    );
    let check: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![definitions],
        root.path().join("check-runs"),
    ));
    let record = check.get("sweep").unwrap().unwrap();
    assert_eq!(record.graph.node("work").unwrap().name, "Renamed");
    assert_eq!(record.defaults.harness.as_deref(), Some("codex"));
    assert_eq!(record.defaults.model.as_deref(), Some("gpt-5"));
}
