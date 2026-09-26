//! Tests for the cloud workflow plane's store adapter.
//!
//! The store is real (a temporary directory) and the operations are the real
//! ones — the point of the adapter is that a socket read and a `medulla
//! workflow` command answer identically, and a mocked store would test the mock
//! instead. Only the harness is a stand-in, because the alternative is starting
//! a coding agent.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::hub::{RunError, TaskOutcome, TaskRequest};
use crate::workflows::FileWorkflowStore;

/// A workflow document with one trigger and one agent step.
fn document(id: &str) -> String {
    json!({
        "id": id,
        "name": "Nightly sweep",
        "description": "sweeps the estate",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "Start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "do it", "agent_ref": "builder" } }
        ],
        "edges": [{ "from_node": "start", "to_node": "work" }]
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

/// A harness stand-in that replies without touching the store, so a copilot
/// turn's plumbing can be exercised without a coding agent.
struct StubHarness {
    reply: String,
    seen: Mutex<Vec<TaskRequest>>,
}

#[async_trait]
impl HarnessDispatch for StubHarness {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        self.seen.lock().unwrap().push(request);
        Ok(TaskOutcome {
            reply: self.reply.clone(),
            usage: crate::protocol::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            harness: None,
            session_id: None,
            transcript: Vec::new(),
        })
    }
}

#[test]
fn list_advertises_every_stored_workflow() {
    let (_root, store) = store();
    ops::create(&store, &document("sweep"), "sweep").expect("creates");

    let bridge = StoreWorkflowBridge::new(store);
    let adverts = bridge.list();

    assert_eq!(adverts.len(), 1);
    assert_eq!(adverts[0].id, "sweep");
    assert_eq!(adverts[0].name, "Nightly sweep");
    assert_eq!(adverts[0].description, "sweeps the estate");
    assert_eq!(adverts[0].node_count, 2);
    assert_eq!(adverts[0].enabled, Some(true));
    assert_eq!(adverts[0].trigger_kind.as_deref(), Some("manual"));
    // Provenance is stamped on the batch, not on each advert.
    assert!(adverts[0].agent_id.is_none());
}

#[test]
fn a_blank_name_or_description_never_reaches_the_wire_as_an_empty_string() {
    // The advert's `name`/`description` skip serialization when blank, and both
    // the backend and the library port therefore treat them as optional. An
    // adapter that substituted `""` would fabricate a name nothing asked for.
    let summary = WorkflowSummary {
        id: "bare".into(),
        name: String::new(),
        description: String::new(),
        enabled: false,
        node_count: 0,
        trigger_kind: None,
        inputs: Vec::new(),
    };
    let wire = serde_json::to_value(descriptor(&summary)).unwrap();

    assert_eq!(wire["id"], "bare");
    assert!(wire.get("name").is_none());
    assert!(wire.get("description").is_none());
    assert_eq!(wire["enabled"], false);
}

#[test]
fn an_unreadable_store_advertises_nothing_rather_than_failing() {
    // A missing workflow directory is the ordinary state of a fresh host.
    let (_root, store) = store();
    assert!(StoreWorkflowBridge::new(store).list().is_empty());
}

#[test]
fn a_store_that_cannot_be_read_is_a_failed_registration_not_an_empty_one() {
    // The two answers the transport tells apart: a batch it can send (which
    // *replaces* whatever the backend held for this socket) and a read it must
    // not send at all. An empty store legitimately registers nothing; a store
    // that failed must leave the previous registration standing.
    let (_root, store) = store();
    assert_eq!(
        StoreWorkflowBridge::new(store.clone()).try_list().unwrap(),
        Vec::new()
    );

    ops::create(&store, &document("sweep"), "sweep").expect("creates");
    let adverts = StoreWorkflowBridge::new(store).try_list().unwrap();
    assert_eq!(adverts.len(), 1);
    assert_eq!(adverts[0].id, "sweep");
}

#[test]
fn a_host_that_was_told_where_it_works_reports_it_on_the_probe() {
    // The transport reads `cwd` off the bridge rather than the core's config,
    // so a host that does not answer here reports no working directory at all.
    let (_root, store) = store();
    assert!(StoreWorkflowBridge::new(store.clone())
        .action_dir()
        .is_none());
    assert_eq!(
        StoreWorkflowBridge::new(store)
            .with_action_dir("/srv/estate")
            .action_dir()
            .as_deref(),
        Some("/srv/estate")
    );
}

#[test]
fn node_count_saturates_instead_of_wrapping() {
    let summary = WorkflowSummary {
        id: "huge".into(),
        name: "Huge".into(),
        description: String::new(),
        enabled: true,
        node_count: usize::MAX,
        trigger_kind: None,
        inputs: Vec::new(),
    };
    assert_eq!(descriptor(&summary).node_count, u32::MAX);
}

#[test]
fn the_reads_answer_exactly_as_the_operations_do() {
    let (_root, store) = store();
    ops::create(&store, &document("sweep"), "sweep").expect("creates");
    let bridge = StoreWorkflowBridge::new(store.clone());

    assert_eq!(
        bridge.get("sweep").unwrap(),
        ops::get(&store, "sweep").unwrap()
    );
    assert_eq!(
        bridge.runs("sweep").unwrap(),
        ops::list_runs(&store, "sweep", ops::StepDetail::Full).unwrap()
    );
    assert_eq!(
        bridge.node_kinds(Some("agent")).unwrap(),
        ops::catalog(Some("agent")).unwrap()
    );
    assert_eq!(
        bridge.node_kinds(None).unwrap(),
        ops::catalog(None).unwrap()
    );
}

#[test]
fn an_unknown_id_comes_back_as_a_sentence_a_model_can_act_on() {
    let (_root, store) = store();
    let bridge = StoreWorkflowBridge::new(store);

    assert_eq!(
        bridge.get("missing").unwrap_err(),
        "no workflow with id 'missing'"
    );
    // An unknown node kind names the ones that exist, so the next attempt can
    // be right.
    let err = bridge.node_kinds(Some("nonsense")).unwrap_err();
    assert!(err.contains("unknown node kind 'nonsense'"), "{err}");
    assert!(err.contains("known kinds:"), "{err}");
}

#[tokio::test]
async fn a_host_without_a_copilot_says_so_instead_of_going_quiet() {
    // Silence here would cost the backend its ten-minute copilot deadline.
    let (_root, store) = store();
    let bridge = StoreWorkflowBridge::new(store);

    assert_eq!(
        bridge
            .copilot("add a step", Some("sweep"))
            .await
            .unwrap_err(),
        "this host has no authoring copilot configured"
    );
}

#[tokio::test]
async fn an_edit_turn_runs_against_the_named_workflow_and_reports_no_false_changes() {
    let (_root, store) = store();
    ops::create(&store, &document("sweep"), "sweep").expect("creates");

    let harness = Arc::new(StubHarness {
        reply: "  looked at it  ".into(),
        seen: Mutex::new(Vec::new()),
    });
    let bridge = StoreWorkflowBridge::new(store)
        .with_copilot(harness.clone(), "worker", None, None)
        .with_agent_id("laptop");

    let outcome = bridge
        .copilot("what does it do?", Some("sweep"))
        .await
        .unwrap();

    assert_eq!(outcome.reply, "looked at it");
    // The stub edited nothing, and the outcome is derived from a re-read of the
    // store rather than from the reply — so it must claim nothing.
    assert!(outcome.changes.is_empty());
    assert!(outcome.created.is_none());
    assert_eq!(bridge.agent_id().as_deref(), Some("laptop"));

    let seen = harness.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].worker_address, "worker");
    // An authoring turn must never carry a workflow: that would *run* the graph
    // the operator is trying to edit.
    assert!(seen[0].workflow.is_none());
    assert!(seen[0].instruction.contains("what does it do?"));
}

#[tokio::test]
async fn a_turn_naming_no_workflow_is_a_create_turn() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness {
        reply: "nothing to build".into(),
        seen: Mutex::new(Vec::new()),
    });
    let bridge =
        StoreWorkflowBridge::new(store).with_copilot(harness.clone(), "worker", None, None);

    let outcome = bridge.copilot("what could I build?", None).await.unwrap();

    // A create turn that created nothing is the right answer to a question, not
    // an error.
    assert_eq!(outcome.reply, "nothing to build");
    assert!(outcome.created.is_none());
    assert_eq!(harness.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn an_edit_turn_against_an_unknown_workflow_never_reaches_the_harness() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness {
        reply: "unreachable".into(),
        seen: Mutex::new(Vec::new()),
    });
    let bridge =
        StoreWorkflowBridge::new(store).with_copilot(harness.clone(), "worker", None, None);

    assert_eq!(
        bridge
            .copilot("edit it", Some("missing"))
            .await
            .unwrap_err(),
        "no workflow with id 'missing'"
    );
    assert!(harness.seen.lock().unwrap().is_empty());
}

#[test]
fn trigger_input_passes_json_through_and_wraps_anything_else() {
    assert_eq!(
        trigger_input(r#"{"repo":"medulla"}"#),
        json!({"repo":"medulla"})
    );
    assert_eq!(trigger_input("ship it"), json!({ "text": "ship it" }));
    // Blank is an empty payload, not a deliberate blank string.
    assert_eq!(trigger_input("   "), json!({}));
}

#[test]
fn cancelling_a_task_nobody_is_running_reports_false_rather_than_pretending() {
    // Process-local by construction: this host cannot cancel another's run, and
    // says so instead of returning a success the caller would misread.
    assert!(!cancel_task_workflow("task-that-was-never-started"));
}
