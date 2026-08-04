//! A workflow learning from its own failure, end to end.
//!
//! The claim under test is the whole loop: a run that fails leaves a durable
//! note without anyone asking, a proposed fix is checked before it is offered,
//! accepting it changes the saved graph *through the ordinary edit path* so it
//! is undoable, and the workflow then runs clean.
//!
//! Offline and process-free. The workflow fails because this test explicitly
//! opts out of `code` nodes — a real, documented operator policy rather than a
//! contrived failure, and one that needs no harness, network, or coding agent.

#![cfg(feature = "workflows")]

use std::collections::HashMap;
use std::sync::Arc;

use medulla::flow_engine::{null_sink, CapabilitySettings, HostServices};
use medulla::workflows::{
    ops, run_workflow, FileWorkflowStore, NoteKind, NoteSource, ProposalStatus, RunContext,
    RunStatus, StoreWorkflowResolver, WorkflowStore,
};
use serde_json::json;

mod feature_workflow_evolve_notes;

/// A store rooted in a scratch home, so nothing here touches a developer's own
/// workflows, runs, or journal.
fn store_in(home: &std::path::Path) -> Arc<dyn WorkflowStore> {
    Arc::new(FileWorkflowStore::with_state(
        vec![home.join("workflows")],
        &home.join("state").join("workflows"),
    ))
}

/// A workflow whose middle step this host will refuse to run.
///
/// `code` nodes are denied by this test's explicit operator policy.
fn install_failing(store: &Arc<dyn WorkflowStore>, id: &str) {
    let document = json!({
        "id": id,
        "name": "Nightly sweep",
        "description": "sweeps",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "code", "name": "Work",
              "config": { "language": "javascript", "source": "return { ok: true };" } }
        ],
        "edges": [{ "from_node": "t", "to_node": "work" }]
    })
    .to_string();
    ops::create(store, &document, id).expect("the workflow installs");
}

/// Run the workflow with no harness available and code execution denied.
async fn run_once(store: &Arc<dyn WorkflowStore>, home: &std::path::Path, id: &str) -> RunStatus {
    let mut settings = CapabilitySettings::rooted_at(home.to_path_buf());
    settings.allow_code = false;
    let max_loop_iterations = settings.max_loop_iterations;
    let context = RunContext {
        store: store.clone(),
        settings: Arc::new(settings),
        services: HostServices {
            dispatch: Arc::new(NoHarness),
            resolver: Arc::new(StoreWorkflowResolver::new(
                store.clone(),
                max_loop_iterations,
            )),
            http_credentials: HashMap::new(),
        },
        sink: null_sink(),
    };
    let run_id = format!("run-{}", uuid::Uuid::new_v4());
    run_workflow(context, id, &run_id, json!({}), Default::default())
        .await
        .expect("the run itself completes, however the graph fares")
        .status
}

/// A dispatch that refuses everything.
///
/// This test never reaches an `agent` node, and a stand-in that quietly
/// succeeded would hide it if a future change made it do so.
struct NoHarness;

#[async_trait::async_trait]
impl medulla::flow_engine::caps::dispatch::HarnessDispatch for NoHarness {
    async fn dispatch(
        &self,
        _request: medulla::hub::TaskRequest,
    ) -> Result<medulla::hub::TaskOutcome, medulla::hub::RunError> {
        Err(medulla::hub::RunError::Transport(
            "no harness in this test".into(),
        ))
    }

    async fn dispatch_with_status(
        &self,
        request: medulla::hub::TaskRequest,
        _status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<medulla::hub::TaskOutcome, medulla::hub::RunError> {
        self.dispatch(request).await
    }

    fn abort_in_flight(&self) {}
}

#[tokio::test]
async fn a_failed_run_teaches_the_workflow_something_without_being_asked() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    install_failing(&store, "sweep");

    assert_eq!(
        run_once(&store, home.path(), "sweep").await,
        RunStatus::Failed
    );

    // Nobody asked for this. No harness ran, no review was triggered, and the
    // note is on disk anyway — that is the floor the whole feature stands on.
    let notes = store.list_notes("sweep").expect("the journal is readable");
    assert_eq!(notes.len(), 1, "exactly one note per failure");
    assert_eq!(notes[0].kind, NoteKind::Observation);
    assert_eq!(notes[0].source, NoteSource::System);
    assert_eq!(notes[0].run_ids.len(), 1, "the note names its evidence");
    assert!(
        notes[0].text.contains("failed"),
        "the note says what happened: {}",
        notes[0].text
    );
}

#[tokio::test]
async fn a_successful_run_teaches_nothing() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    let document = json!({
        "id": "fine",
        "name": "Fine",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "transform", "name": "Work",
              "config": { "set": { "ok": true } } }
        ],
        "edges": [{ "from_node": "t", "to_node": "work" }]
    })
    .to_string();
    ops::create(&store, &document, "fine").expect("installs");

    assert_eq!(
        run_once(&store, home.path(), "fine").await,
        RunStatus::Succeeded
    );

    // A journal that filled up with "it worked" would bury the entries that
    // matter and evict them at the cap.
    assert!(store.list_notes("fine").expect("readable").is_empty());
}

#[tokio::test]
async fn a_run_records_the_summary_and_diagnosis_it_produced() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    install_failing(&store, "sweep");
    run_once(&store, home.path(), "sweep").await;

    let run = store
        .list_runs("sweep")
        .expect("readable")
        .into_iter()
        .next()
        .expect("a run");

    // Both are what a later review reads instead of re-deriving them.
    assert!(run.summary.is_some(), "the run says what it did");
    assert!(run.diagnosis.is_some(), "and what was wrong beyond failing");
}

#[tokio::test]
async fn a_proposal_is_checked_offered_applied_and_undoable() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    install_failing(&store, "sweep");
    run_once(&store, home.path(), "sweep").await;
    let run_id = store.list_runs("sweep").expect("readable")[0].id.clone();

    // The fix: stop asking this host to run code it will not run.
    let proposed = ops::propose(
        &store,
        "sweep",
        "This host denies `code` nodes, so the step can never run here. Do the same \
         work as a transform.",
        &json!([
            { "op": "remove_node", "id": "work" },
            { "op": "add_node", "node": {
                "id": "work", "kind": "transform", "name": "Work",
                "config": { "set": { "ok": true } } } },
            { "op": "add_edge", "edge": { "from_node": "t", "to_node": "work" } }
        ]),
        vec![run_id.clone()],
        Vec::new(),
    )
    .await
    .expect("proposing does not fail");
    assert_eq!(proposed["ok"], json!(true), "{proposed:#}");

    // Nothing has changed yet. This is the whole point of a proposal.
    let before = store.get("sweep").expect("readable").expect("still there");
    assert_eq!(
        before.graph.nodes[1].kind,
        tinyflows::model::NodeKind::Code,
        "a proposal must not touch the saved graph"
    );

    let proposal_id = proposed["proposed"].as_str().expect("an id").to_string();
    ops::accept_proposal(&store, &proposal_id).expect("accepting applies it");

    let after = store.get("sweep").expect("readable").expect("still there");
    assert_eq!(
        after.graph.nodes[1].kind,
        tinyflows::model::NodeKind::Transform
    );

    // Accepting went through the ordinary edit path, so the previous graph was
    // snapshotted and the operator's undo key reaches it.
    let history = store.list_revisions("sweep").expect("readable");
    assert_eq!(history.len(), 1, "an accepted proposal is undoable");
    assert_eq!(
        history[0].record.graph.nodes[1].kind,
        tinyflows::model::NodeKind::Code
    );

    // And the workflow now runs.
    assert_eq!(
        run_once(&store, home.path(), "sweep").await,
        RunStatus::Succeeded
    );
}

#[tokio::test]
async fn accepting_records_what_landed_and_marks_the_proposal_spent() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    install_failing(&store, "sweep");
    run_once(&store, home.path(), "sweep").await;

    let proposed = ops::propose(
        &store,
        "sweep",
        "run it as a transform instead",
        &json!([
            { "op": "remove_node", "id": "work" },
            { "op": "add_node", "node": {
                "id": "work", "kind": "transform", "name": "Work",
                "config": { "set": { "ok": true } } } },
            { "op": "add_edge", "edge": { "from_node": "t", "to_node": "work" } }
        ]),
        Vec::new(),
        Vec::new(),
    )
    .await
    .expect("proposing does not fail");
    let proposal_id = proposed["proposed"].as_str().expect("an id").to_string();
    ops::accept_proposal(&store, &proposal_id).expect("accepting applies it");

    let stored = store
        .get_proposal(&proposal_id)
        .expect("readable")
        .expect("still there");
    assert_eq!(stored.status, ProposalStatus::Accepted);
    assert!(stored.decided_at.is_some());

    // A second accept must not re-apply an edit that already landed.
    assert!(ops::accept_proposal(&store, &proposal_id).is_err());

    // What landed is itself recorded, so the next review knows it was tried.
    let notes = store.list_notes("sweep").expect("readable");
    assert!(
        notes.iter().any(|note| note.kind == NoteKind::Fix),
        "an applied change is recorded as a fix"
    );
}

#[tokio::test]
async fn a_rejected_proposal_leaves_the_reason_behind() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    install_failing(&store, "sweep");

    let proposed = ops::propose(
        &store,
        "sweep",
        "delete the step entirely",
        &json!([{ "op": "remove_node", "id": "work" }]),
        Vec::new(),
        Vec::new(),
    )
    .await
    .expect("proposing does not fail");
    let proposal_id = proposed["proposed"].as_str().expect("an id").to_string();

    ops::reject_proposal(
        &store,
        &proposal_id,
        "the step is the point of the workflow",
    )
    .expect("rejecting works");

    // The reason is what stops the next review proposing the same thing.
    let notes = store.list_notes("sweep").expect("readable");
    let rejection = notes
        .iter()
        .find(|note| note.kind == NoteKind::Rejection)
        .expect("a rejection is recorded");
    assert!(rejection.text.contains("the point of the workflow"));
    assert!(
        rejection.pinned,
        "an operator's decision must survive the journal's cap"
    );

    // And the graph is untouched.
    let record = store.get("sweep").expect("readable").expect("still there");
    assert_eq!(record.graph.nodes.len(), 2);
}

#[tokio::test]
async fn a_proposal_written_against_an_older_graph_is_refused_as_stale() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    install_failing(&store, "sweep");

    let proposed = ops::propose(
        &store,
        "sweep",
        "rename the step",
        &json!([{ "op": "set_node_name", "id": "work", "name": "Renamed" }]),
        Vec::new(),
        Vec::new(),
    )
    .await
    .expect("proposing does not fail");
    let proposal_id = proposed["proposed"].as_str().expect("an id").to_string();

    // Someone else edits the workflow in the meantime.
    ops::apply_ops(
        &store,
        "sweep",
        &json!([{ "op": "set_node_name", "id": "work", "name": "Edited by hand" }]),
    )
    .expect("the hand edit lands");

    let refused = ops::accept_proposal(&store, &proposal_id);
    assert!(
        refused.is_err(),
        "ops are positional; a moved graph is not a merge"
    );

    let stored = store
        .get_proposal(&proposal_id)
        .expect("readable")
        .expect("still there");
    assert_eq!(stored.status, ProposalStatus::Stale);
    // Recorded rather than merely refused, so it stops being offered.
    assert!(stored.decision_reason.is_some());

    let record = store.get("sweep").expect("readable").expect("still there");
    assert_eq!(
        record.graph.nodes[1].name, "Edited by hand",
        "the hand edit survives"
    );
}

#[tokio::test]
async fn a_proposal_that_will_not_apply_is_kept_as_evidence() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    install_failing(&store, "sweep");

    let proposed = ops::propose(
        &store,
        "sweep",
        "patch a node that does not exist",
        &json!([{ "op": "set_node_name", "id": "nonexistent", "name": "x" }]),
        Vec::new(),
        Vec::new(),
    )
    .await
    .expect("a bad proposal is an outcome, not an error");

    assert_eq!(proposed["ok"], json!(false));
    let proposal_id = proposed["proposed"].as_str().expect("an id");

    // Kept, not dropped: without it a review re-derives the same broken edit
    // every time the same history comes back around.
    let stored = store
        .get_proposal(proposal_id)
        .expect("readable")
        .expect("it was still stored");
    let check = stored.verification.expect("it was checked");
    assert!(!check.ok);
    assert!(!check.messages.is_empty(), "and it says why");

    // It cannot be applied.
    assert!(ops::accept_proposal(&store, proposal_id).is_err());
}

#[tokio::test]
async fn a_new_proposal_supersedes_the_one_still_waiting() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    install_failing(&store, "sweep");

    let first = ops::propose(
        &store,
        "sweep",
        "first idea",
        &json!([{ "op": "set_node_name", "id": "work", "name": "First" }]),
        Vec::new(),
        Vec::new(),
    )
    .await
    .expect("proposing works");
    let second = ops::propose(
        &store,
        "sweep",
        "better idea",
        &json!([{ "op": "set_node_name", "id": "work", "name": "Second" }]),
        Vec::new(),
        Vec::new(),
    )
    .await
    .expect("proposing works");

    // An operator asked to adjudicate two competing proposals stops reading
    // them; the older one was reasoned from less evidence anyway.
    let older = store
        .get_proposal(first["proposed"].as_str().expect("an id"))
        .expect("readable")
        .expect("still there");
    assert_eq!(older.status, ProposalStatus::Stale);

    let newer = store
        .get_proposal(second["proposed"].as_str().expect("an id"))
        .expect("readable")
        .expect("still there");
    assert_eq!(newer.status, ProposalStatus::Pending);
}

#[tokio::test]
async fn notes_and_proposals_are_kept_apart_by_workflow() {
    let home = tempfile::tempdir().expect("a temp home");
    let store = store_in(home.path());
    install_failing(&store, "sweep");
    install_failing(&store, "deploy");

    run_once(&store, home.path(), "sweep").await;
    ops::propose(
        &store,
        "sweep",
        "an idea about sweep",
        &json!([{ "op": "set_node_name", "id": "work", "name": "Renamed" }]),
        Vec::new(),
        Vec::new(),
    )
    .await
    .expect("proposing works");

    assert_eq!(store.list_notes("sweep").expect("readable").len(), 1);
    assert!(store.list_notes("deploy").expect("readable").is_empty());
    assert_eq!(store.list_proposals("sweep").expect("readable").len(), 1);
    assert!(store.list_proposals("deploy").expect("readable").is_empty());
}
