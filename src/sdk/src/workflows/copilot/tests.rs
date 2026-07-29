//! Tests for one copilot turn.
//!
//! The store is real (a temporary directory) and the diff is real; only the
//! harness is a stand-in, because the alternative is starting a coding agent.
//! The stand-in is where the *edits* come from too: a real copilot edits through
//! the MCP tools, and a stub that writes to the same store is indistinguishable
//! from that as far as this module is concerned.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;

use super::*;
use crate::hub::{RunError, TaskOutcome};
use crate::workflows::{FileWorkflowStore, WorkflowRecord};

/// A workflow document with one trigger and one agent step.
fn document(id: &str) -> WorkflowRecord {
    WorkflowRecord {
        id: id.to_string(),
        name: "Nightly sweep".into(),
        description: "sweeps".into(),
        enabled: true,
        graph: serde_json::from_value(json!({
            "name": "Nightly sweep",
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "work", "kind": "agent", "name": "Work",
                  "config": { "prompt": "do it" } },
            ],
            "edges": [{ "from_node": "start", "to_node": "work" }],
        }))
        .expect("graph parses"),
        source_path: None,
    }
}

/// What a stand-in harness does to the store before it replies — standing in
/// for the tool calls a real copilot turn would have made.
type StoreEdit = Box<dyn Fn(&dyn WorkflowStore) + Send + Sync>;

/// A harness stand-in: replies with `reply`, optionally editing the store first
/// (which is what a real copilot's tool calls would have done), and reports
/// `statuses` as progress along the way.
struct StubHarness {
    reply: String,
    statuses: Vec<String>,
    edit: Option<StoreEdit>,
    seen: Mutex<Vec<TaskRequest>>,
    store: Arc<dyn WorkflowStore>,
}

impl StubHarness {
    fn new(store: Arc<dyn WorkflowStore>, reply: &str) -> Self {
        Self {
            reply: reply.to_string(),
            statuses: Vec::new(),
            edit: None,
            seen: Mutex::new(Vec::new()),
            store,
        }
    }
}

#[async_trait]
impl HarnessDispatch for StubHarness {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        self.seen.lock().unwrap().push(request);
        if let Some(edit) = &self.edit {
            edit(self.store.as_ref());
        }
        Ok(TaskOutcome {
            reply: self.reply.clone(),
            usage: crate::tinyplace::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            harness: None,
        })
    }

    async fn dispatch_with_status(
        &self,
        request: TaskRequest,
        status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        if let Some(status) = &status {
            for line in &self.statuses {
                let _ = status.send(line.clone());
            }
        }
        self.dispatch(request).await
    }
}

/// A dispatch that always fails, for the unhappy path.
struct BrokenHarness;

#[async_trait]
impl HarnessDispatch for BrokenHarness {
    async fn dispatch(&self, _request: TaskRequest) -> Result<TaskOutcome, RunError> {
        Err(RunError::Worker("no harness installed".into()))
    }
}

/// A dispatch that fails with one chosen [`RunError`], for asserting that the
/// four kinds of failure stay distinguishable to the caller.
struct FailingHarness(RunError);

#[async_trait]
impl HarnessDispatch for FailingHarness {
    async fn dispatch(&self, _request: TaskRequest) -> Result<TaskOutcome, RunError> {
        Err(self.0.clone())
    }
}

/// A store whose listing always fails.
///
/// Stands in for a workflows directory that has gone unreadable mid-session —
/// permissions changed, a mount dropped — which is the case where swallowing the
/// error used to make every existing workflow look newly created.
struct UnreadableStore;

impl UnreadableStore {
    fn failure() -> WorkflowError {
        WorkflowError::Io {
            path: "workflows".into(),
            source: std::io::Error::other("unreadable"),
        }
    }
}

impl WorkflowStore for UnreadableStore {
    fn list(&self) -> Result<Vec<crate::workflows::WorkflowSummary>, WorkflowError> {
        Err(Self::failure())
    }
    fn get(&self, _id: &str) -> Result<Option<WorkflowRecord>, WorkflowError> {
        Err(Self::failure())
    }
    fn save(&self, _record: &WorkflowRecord) -> Result<(), WorkflowError> {
        Err(Self::failure())
    }
    fn delete(&self, _id: &str) -> Result<(), WorkflowError> {
        Err(Self::failure())
    }
    fn record_run(&self, _run: &crate::workflows::RunRecord) -> Result<(), WorkflowError> {
        Err(Self::failure())
    }
    fn get_run(&self, _run_id: &str) -> Result<Option<crate::workflows::RunRecord>, WorkflowError> {
        Err(Self::failure())
    }
    fn list_runs(
        &self,
        _workflow_id: &str,
    ) -> Result<Vec<crate::workflows::RunRecord>, WorkflowError> {
        Err(Self::failure())
    }
}

/// A store over a temporary directory, holding `sweep`.
fn store() -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    let root = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    store.save(&document("sweep")).expect("save");
    (root, store as Arc<dyn WorkflowStore>)
}

/// A session dispatching to `harness`.
fn session(store: Arc<dyn WorkflowStore>, harness: Arc<dyn HarnessDispatch>) -> CopilotSession {
    CopilotSession {
        store,
        dispatch: harness,
        worker_address: "worker".into(),
        provider: None,
        model: None,
    }
}

#[tokio::test]
async fn a_turn_dispatches_the_operator_instruction_inside_a_scoped_prompt() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "looked at it"));
    let session = session(store, harness.clone());

    session
        .turn("sweep", "what does this do?", None)
        .await
        .expect("turn");

    let seen = harness.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].instruction.contains("what does this do?"));
    assert!(seen[0].instruction.contains("id: sweep"));
    assert_eq!(seen[0].worker_address, "worker");
    assert!(
        seen[0].workflow.is_none(),
        "an authoring turn must never run the graph it is editing"
    );
}

#[tokio::test]
async fn each_turn_gets_its_own_task_id_so_two_are_never_deduped() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "ok"));
    let session = session(store, harness.clone());

    session.turn("sweep", "one", None).await.expect("first");
    session.turn("sweep", "two", None).await.expect("second");

    let seen = harness.seen.lock().unwrap();
    assert_ne!(seen[0].task_id, seen[1].task_id);
    assert_eq!(
        seen[0].abort_id, seen[0].task_id,
        "the turn is abortable by the id it was dispatched under"
    );
}

#[tokio::test]
async fn a_turn_that_changes_nothing_reports_no_changes() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "it sweeps the repo"));
    let session = session(store, harness);

    let outcome = session
        .turn("sweep", "explain it", None)
        .await
        .expect("turn");

    assert_eq!(outcome.reply, "it sweeps the repo");
    assert!(outcome.changes.is_empty());
}

#[tokio::test]
async fn a_turn_reports_what_the_store_actually_holds_afterwards() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "added the step");
    harness.edit = Some(Box::new(|store| {
        let mut record = store.get("sweep").unwrap().unwrap();
        record.graph.nodes.push(
            serde_json::from_value(json!({
                "id": "notify", "kind": "tool_call", "name": "Notify",
                "config": { "tool": "slack" },
            }))
            .unwrap(),
        );
        record.graph.edges.push(
            serde_json::from_value(json!({ "from_node": "work", "to_node": "notify" })).unwrap(),
        );
        store.save(&record).unwrap();
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session
        .turn("sweep", "notify slack at the end", None)
        .await
        .expect("turn");

    assert!(
        outcome
            .changes
            .contains(&"+ node notify (tool_call)".to_string()),
        "{:?}",
        outcome.changes
    );
    assert!(
        outcome
            .changes
            .contains(&"+ edge work → notify".to_string()),
        "{:?}",
        outcome.changes
    );
}

#[tokio::test]
async fn an_agent_that_claims_an_edit_it_did_not_make_is_not_believed() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "I added a Slack step!"));
    let session = session(store, harness);

    let outcome = session
        .turn("sweep", "add slack", None)
        .await
        .expect("turn");

    assert!(
        outcome.changes.is_empty(),
        "the store is the only witness: {:?}",
        outcome.changes
    );
}

#[tokio::test]
async fn progress_frames_reach_the_caller_as_they_arrive() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "done");
    harness.statuses = vec!["reading the graph".into(), "applying ops".into()];
    let session = session(store, Arc::new(harness));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    session.turn("sweep", "go", Some(tx)).await.expect("turn");

    let mut seen = Vec::new();
    while let Ok(line) = rx.try_recv() {
        seen.push(line);
    }
    assert_eq!(seen, vec!["reading the graph", "applying ops"]);
}

#[tokio::test]
async fn a_turn_against_an_unknown_workflow_fails_before_dispatching_anything() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "ok"));
    let session = session(store, harness.clone());

    let err = session
        .turn("absent", "go", None)
        .await
        .expect_err("no such workflow");

    assert!(matches!(err, WorkflowError::NotFound(_)), "{err}");
    assert!(
        harness.seen.lock().unwrap().is_empty(),
        "nothing should have been dispatched"
    );
}

#[tokio::test]
async fn a_failed_dispatch_surfaces_the_harness_error() {
    let (_root, store) = store();
    let session = session(store, Arc::new(BrokenHarness));

    let err = session.turn("sweep", "go", None).await.expect_err("broken");

    assert!(err.to_string().contains("no harness installed"), "{err}");
}

#[tokio::test]
async fn a_workflow_the_turn_deleted_is_reported_as_a_removal() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "removed it");
    harness.edit = Some(Box::new(|store| {
        store.delete("sweep").unwrap();
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session
        .turn("sweep", "delete it", None)
        .await
        .expect("turn");

    assert_eq!(outcome.reply, "removed it");
    // Not empty: the caller only refreshes its catalogue when something
    // changed, so a silent deletion leaves the workflow on screen.
    assert_eq!(outcome.changes, vec!["− workflow sweep".to_string()]);
}

#[tokio::test]
async fn a_turn_that_only_edited_the_description_still_reports_a_change() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "reworded it");
    harness.edit = Some(Box::new(|store| {
        let mut record = store.get("sweep").unwrap().unwrap();
        record.description = "sweeps the repo every night".into();
        store.save(&record).unwrap();
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session
        .turn("sweep", "describe it better", None)
        .await
        .expect("turn");

    assert_eq!(outcome.changes, vec!["~ description".to_string()]);
}

#[tokio::test]
async fn a_turn_that_disabled_the_workflow_reports_it() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "turned it off");
    harness.edit = Some(Box::new(|store| {
        let mut record = store.get("sweep").unwrap().unwrap();
        record.enabled = false;
        store.save(&record).unwrap();
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session
        .turn("sweep", "stop it running", None)
        .await
        .expect("turn");

    assert_eq!(outcome.changes, vec!["~ disabled".to_string()]);
}

#[tokio::test]
async fn dispatch_failures_keep_the_shape_the_hub_reported() {
    let (_root, store) = store();

    for (run_error, expected) in [
        (RunError::Timeout, "did not respond in time"),
        (RunError::Aborted, "aborted"),
        (RunError::Transport("relay is down".into()), "relay is down"),
    ] {
        let session = session(store.clone(), Arc::new(FailingHarness(run_error)));
        let err = session.turn("sweep", "go", None).await.expect_err("fails");
        assert!(err.to_string().contains(expected), "{err}");
    }
}

#[tokio::test]
async fn a_reply_is_trimmed_so_it_does_not_open_the_pane_with_blank_lines() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "\n\n  done  \n"));
    let session = session(store, harness);

    let outcome = session.turn("sweep", "go", None).await.expect("turn");

    assert_eq!(outcome.reply, "done");
}

#[tokio::test]
async fn a_create_turn_reports_the_workflow_that_appeared() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "built it");
    // Standing in for the `workflow_create` call a real copilot would make.
    harness.edit = Some(Box::new(|store| {
        store.save(&document("fresh")).expect("save");
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session
        .create("summarise new issues daily", None)
        .await
        .expect("create");

    assert_eq!(outcome.created.as_deref(), Some("fresh"));
    assert_eq!(outcome.changes, vec!["+ workflow fresh".to_string()]);
    assert_eq!(outcome.reply, "built it");
}

#[tokio::test]
async fn a_create_turn_is_told_to_create_rather_than_to_edit() {
    let (_root, store) = store();
    let harness = Arc::new(StubHarness::new(store.clone(), "ok"));
    let session = session(store, harness.clone());

    session.create("something new", None).await.expect("create");

    let seen = harness.seen.lock().unwrap();
    let prompt = &seen[0].instruction;
    assert!(prompt.contains("workflow_create"), "{prompt}");
    assert!(prompt.contains("something new"), "{prompt}");
    // The existing workflow must not be named: this turn only adds, and naming
    // one is how an agent talks itself into editing it.
    assert!(!prompt.contains("sweep"), "{prompt}");
}

#[tokio::test]
async fn a_create_turn_that_built_nothing_is_not_a_failure() {
    let (_root, store) = store();
    // No edit: the agent answered a question instead of building.
    let harness = Arc::new(StubHarness::new(store.clone(), "you could try…"));
    let session = session(store, harness);

    let outcome = session
        .create("what could I build?", None)
        .await
        .expect("create");

    assert_eq!(outcome.created, None);
    assert!(outcome.changes.is_empty());
    assert_eq!(outcome.reply, "you could try…");
}

#[tokio::test]
async fn a_create_turn_ignores_a_workflow_that_was_already_there() {
    let (_root, store) = store();
    // The stub rewrites the *existing* workflow rather than adding one.
    let mut harness = StubHarness::new(store.clone(), "touched it");
    harness.edit = Some(Box::new(|store| {
        let mut record = document("sweep");
        record.name = "Renamed".into();
        store.save(&record).expect("save");
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session
        .create("make something", None)
        .await
        .expect("create");

    // Nothing is new, so nothing is reported as created — the comparison is by
    // id against the catalogue, not "did the store change at all".
    assert_eq!(outcome.created, None);
    assert!(outcome.changes.is_empty());
}

#[tokio::test]
async fn a_create_turn_reports_every_workflow_it_made_not_only_the_first() {
    let (_root, store) = store();
    let mut harness = StubHarness::new(store.clone(), "built them");
    harness.edit = Some(Box::new(|store| {
        store.save(&document("alpha")).expect("save");
        store.save(&document("beta")).expect("save");
    }));
    let session = session(store, Arc::new(harness));

    let outcome = session.create("build two", None).await.expect("create");

    // Both reported — an unmentioned second workflow is one the operator has
    // and cannot see. The first by id is offered as the one to select.
    assert_eq!(
        outcome.changes,
        vec![
            "+ workflow alpha".to_string(),
            "+ workflow beta".to_string()
        ]
    );
    assert_eq!(outcome.created.as_deref(), Some("alpha"));
}

#[tokio::test]
async fn a_create_turn_fails_rather_than_guessing_when_the_catalogue_cannot_be_read() {
    let store: Arc<dyn WorkflowStore> = Arc::new(UnreadableStore);
    let harness = Arc::new(StubHarness::new(store.clone(), "ok"));
    let session = session(store, harness.clone());

    let err = session
        .create("build me one", None)
        .await
        .expect_err("fails");

    assert!(matches!(err, WorkflowError::Io { .. }), "{err}");
    assert!(
        harness.seen.lock().unwrap().is_empty(),
        "an unreadable catalogue must stop the turn before it dispatches: a \
         silent empty set makes every existing workflow look newly created"
    );
}

#[tokio::test]
async fn a_create_turn_that_cannot_reach_a_harness_fails() {
    let (_root, store) = store();
    let session = session(store, Arc::new(BrokenHarness));

    let err = session.create("build me one", None).await.unwrap_err();

    assert!(err.to_string().contains("no harness installed"));
}
