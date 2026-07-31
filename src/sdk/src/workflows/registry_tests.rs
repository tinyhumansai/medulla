//! Tests for [`StoreWorkflowResolver`] — in particular, that a resolved
//! child's own `defaults` reach its `agent` nodes rather than being silently
//! dropped once the parent run's shared capabilities take over.

use std::sync::Arc;

use serde_json::json;
use tinyflows::caps::WorkflowResolver;

use super::StoreWorkflowResolver;
use crate::workflows::store::parse_workflow;
use crate::workflows::{FileWorkflowStore, WorkflowStore};

/// A store rooted at a fresh temporary directory, for one test's use.
fn store() -> (tempfile::TempDir, Arc<FileWorkflowStore>) {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    (root, store)
}

/// Save `document` (with its own declared `id`) into `store`.
fn install(store: &Arc<FileWorkflowStore>, document: serde_json::Value) {
    let text = document.to_string();
    let id = document["id"].as_str().unwrap().to_string();
    let record = parse_workflow(&text, &id).expect("valid fixture");
    store.save(&record).expect("saves");
}

#[tokio::test]
async fn a_resolved_child_s_defaults_reach_its_agent_nodes() {
    let (_root, store) = store();
    install(
        &store,
        json!({
            "id": "child",
            "name": "Child",
            "defaults": { "harness": "codex", "model": "gpt-5-codex" },
            "nodes": [
                { "id": "t", "kind": "trigger", "name": "start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "work", "kind": "agent", "name": "Work",
                  "config": { "prompt": "go" } }
            ],
            "edges": [{ "from_node": "t", "to_node": "work" }]
        }),
    );

    let resolver = StoreWorkflowResolver::new(store);
    let graph = resolver.resolve("child").await.expect("resolves");

    let agent_node = graph
        .nodes
        .iter()
        .find(|n| n.id == "work")
        .expect("agent node present");
    assert_eq!(agent_node.config["harness"], json!("codex"));
    assert_eq!(agent_node.config["model"], json!("gpt-5-codex"));
}

#[tokio::test]
async fn a_node_that_names_its_own_harness_is_not_overridden_by_the_child_s_defaults() {
    let (_root, store) = store();
    install(
        &store,
        json!({
            "id": "child",
            "name": "Child",
            "defaults": { "harness": "codex", "model": "gpt-5-codex" },
            "nodes": [
                { "id": "t", "kind": "trigger", "name": "start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "work", "kind": "agent", "name": "Work",
                  "config": { "prompt": "go", "harness": "claude" } }
            ],
            "edges": [{ "from_node": "t", "to_node": "work" }]
        }),
    );

    let resolver = StoreWorkflowResolver::new(store);
    let graph = resolver.resolve("child").await.expect("resolves");

    let agent_node = graph
        .nodes
        .iter()
        .find(|n| n.id == "work")
        .expect("agent node present");
    assert_eq!(agent_node.config["harness"], json!("claude"));
    // The child's Codex model must not follow the step onto Claude Code.
    assert!(agent_node.config.get("model").is_none());
}

#[tokio::test]
async fn a_child_with_no_defaults_is_returned_unmodified() {
    let (_root, store) = store();
    install(
        &store,
        json!({
            "id": "child",
            "name": "Child",
            "nodes": [
                { "id": "t", "kind": "trigger", "name": "start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "work", "kind": "agent", "name": "Work",
                  "config": { "prompt": "go" } }
            ],
            "edges": [{ "from_node": "t", "to_node": "work" }]
        }),
    );

    let resolver = StoreWorkflowResolver::new(store);
    let graph = resolver.resolve("child").await.expect("resolves");

    let agent_node = graph
        .nodes
        .iter()
        .find(|n| n.id == "work")
        .expect("agent node present");
    assert!(agent_node.config.get("harness").is_none());
    assert!(agent_node.config.get("model").is_none());
}
