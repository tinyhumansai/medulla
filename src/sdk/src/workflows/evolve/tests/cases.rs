//! Tests for the evolution pass.
//!
//! The harness is a stand-in; the store, the journal, the verification, and the
//! decision are all real. That split matters, because everything worth
//! asserting here is about what survives when the *agent* does the wrong thing
//! — replies with prose, writes nothing, proposes something broken, or never
//! answers at all.

pub(super) use std::sync::{Arc, Mutex};

use async_trait::async_trait;
pub(super) use serde_json::json;

pub(super) use super::super::*;
pub(super) use crate::flow_engine::caps::dispatch::HarnessDispatch;
pub(super) use crate::hub::{RunError, TaskOutcome, TaskRequest};
pub(super) use crate::workflows::{
    mint_note_id, mint_proposal_id, new_run_record, ops, FileWorkflowStore, NoteKind, NoteSource,
    ProposalStatus, RunRecord, RunStatus, RunStep, WorkflowNote, WorkflowStore,
};

/// What a stand-in harness does to the store before it replies, standing in for
/// the tool calls a real review turn would have made.
type StoreEdit = Box<dyn Fn(&Arc<dyn WorkflowStore>) + Send + Sync>;

/// A harness stand-in for a review turn.
struct StubReviewer {
    reply: String,
    edit: Option<StoreEdit>,
    store: Arc<dyn WorkflowStore>,
    seen: Mutex<Vec<TaskRequest>>,
}

#[async_trait]
impl HarnessDispatch for StubReviewer {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        self.seen.lock().unwrap().push(request);
        if let Some(edit) = &self.edit {
            edit(&self.store);
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
}

/// A dispatch that never reaches a harness.
struct NoHarness;

#[async_trait]
impl HarnessDispatch for NoHarness {
    async fn dispatch(&self, _request: TaskRequest) -> Result<TaskOutcome, RunError> {
        Err(RunError::Transport("no harness installed".into()))
    }
}

/// A real store in a scratch directory, holding one workflow and one failed run.
///
/// Each test names its own workflow. The claim registry is process-global by
/// design — it debounces a failure storm across a whole host — so two tests
/// sharing an id would have one skip the other's pass and fail for a reason
/// that has nothing to do with what it is testing.
pub(super) fn fixture(id: &str) -> (tempfile::TempDir, Arc<dyn WorkflowStore>, RunRecord) {
    let home = tempfile::tempdir().expect("a temp dir");
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::with_state(
        vec![home.path().join("workflows")],
        &home.path().join("state"),
    ));
    let document = json!({
        "id": id,
        "name": "Nightly sweep",
        "description": "sweeps",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "transform", "name": "Work",
              "config": { "set": { "ok": true } } }
        ],
        "edges": [{ "from_node": "t", "to_node": "work" }]
    })
    .to_string();
    ops::create(&store, &document, id).expect("installs");

    let mut run = new_run_record("run-1", id, 1);
    run.status = RunStatus::Failed;
    run.finished_at = Some(2);
    run.error = Some("the worker refused the task".into());
    run.summary = Some("workflow sweep failed at work".into());
    run.steps = vec![RunStep {
        node_id: "work".into(),
        status: "error".into(),
        duration_ms: 1,
        diagnostics: Vec::new(),
    }];
    store.record_run(&run).expect("the run records");

    (home, store, run)
}

/// A session over `store` whose harness is `dispatch`.
pub(super) fn session(
    store: Arc<dyn WorkflowStore>,
    dispatch: Arc<dyn HarnessDispatch>,
) -> EvolveSession {
    EvolveSession {
        store,
        dispatch,
        worker_address: "test-worker".into(),
        provider: None,
        model: None,
        conversation: "test".into(),
        config: EvolveConfig::default(),
    }
}

/// A stub that replies without touching the store.
pub(super) fn silent(store: &Arc<dyn WorkflowStore>, reply: &str) -> Arc<dyn HarnessDispatch> {
    Arc::new(StubReviewer {
        reply: reply.to_string(),
        edit: None,
        store: store.clone(),
        seen: Mutex::new(Vec::new()),
    })
}

#[tokio::test]
async fn an_agent_that_writes_nothing_still_leaves_the_system_note() {
    let (_home, store, run) = fixture("an-agent-that-writes-nothing-still-leave");

    let outcome = session(
        store.clone(),
        silent(&store, "Looks like a transient fault."),
    )
    .evolve(
        "an-agent-that-writes-nothing-still-leave",
        EvolveTrigger::Failure(run.id.clone()),
        None,
    )
    .await
    .expect("the pass completes");

    // The floor the feature stands on: the agent contributed nothing but prose,
    // and the workflow still knows more than it did.
    assert_eq!(outcome.notes.len(), 1);
    assert_eq!(outcome.notes[0].source, NoteSource::System);
    assert_eq!(
        store
            .list_notes("an-agent-that-writes-nothing-still-leave")
            .expect("readable")
            .len(),
        1
    );
    assert_eq!(outcome.reply, "Looks like a transient fault.");
}

#[tokio::test]
async fn the_note_is_written_even_when_no_harness_answers() {
    let (_home, store, run) = fixture("the-note-is-written-even-when-no-harness");

    let failed = session(store.clone(), Arc::new(NoHarness))
        .evolve(
            "the-note-is-written-even-when-no-harness",
            EvolveTrigger::Failure(run.id.clone()),
            None,
        )
        .await;

    assert!(failed.is_err(), "a dispatch failure is still a failure");
    // But not at the cost of the observation. This is why the note is written
    // before anything is dispatched rather than after the turn returns.
    let notes = store
        .list_notes("the-note-is-written-even-when-no-harness")
        .expect("readable");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].kind, NoteKind::Observation);
}

#[tokio::test]
async fn a_manual_review_writes_no_system_note_because_nothing_failed() {
    let (_home, store, _run) = fixture("a-manual-review-writes-no-system-note-be");

    let outcome = session(store.clone(), silent(&store, "Nothing to change."))
        .evolve(
            "a-manual-review-writes-no-system-note-be",
            EvolveTrigger::Manual,
            None,
        )
        .await
        .expect("the pass completes");

    // A manual review is a question, not an event. Recording "someone asked"
    // would fill the journal with entries carrying no evidence.
    assert!(outcome.notes.is_empty());
}

#[tokio::test]
async fn the_brief_carries_the_journal_and_the_failing_run() {
    let (_home, store, run) = fixture("the-brief-carries-the-journal-and-the-fa");
    store
        .append_note(&WorkflowNote {
            id: mint_note_id(1),
            workflow_id: "the-brief-carries-the-journal-and-the-fa".into(),
            kind: NoteKind::Constraint,
            text: "the work step must stay last".into(),
            recorded_at: 1,
            source: NoteSource::Operator,
            run_ids: Vec::new(),
            superseded_by: None,
            pinned: true,
        })
        .expect("the note records");

    let stub = Arc::new(StubReviewer {
        reply: "ok".into(),
        edit: None,
        store: store.clone(),
        seen: Mutex::new(Vec::new()),
    });
    session(store.clone(), stub.clone())
        .evolve(
            "the-brief-carries-the-journal-and-the-fa",
            EvolveTrigger::Failure(run.id.clone()),
            None,
        )
        .await
        .expect("the pass completes");

    let seen = stub.seen.lock().unwrap();
    let prompt = &seen[0].instruction;
    assert!(prompt.contains("the work step must stay last"), "{prompt}");
    assert!(prompt.contains("run-1"), "{prompt}");
    assert!(prompt.contains("the worker refused the task"), "{prompt}");
    // The turn must not be routed as a workflow run: that would execute the
    // graph it was asked to review.
    assert!(seen[0].workflow.is_none());
}

#[tokio::test]
async fn a_superseded_note_is_kept_out_of_the_brief() {
    let (_home, store, run) = fixture("a-superseded-note-is-kept-out-of-the-bri");
    let stale = WorkflowNote {
        id: mint_note_id(1),
        workflow_id: "a-superseded-note-is-kept-out-of-the-bri".into(),
        kind: NoteKind::Hypothesis,
        text: "the timeout is too short".into(),
        recorded_at: 1,
        source: NoteSource::System,
        run_ids: Vec::new(),
        superseded_by: None,
        pinned: false,
    };
    store.append_note(&stale).expect("records");
    store
        .supersede_note(
            "a-superseded-note-is-kept-out-of-the-bri",
            &stale.id,
            "something-newer",
        )
        .expect("supersedes");

    let stub = Arc::new(StubReviewer {
        reply: "ok".into(),
        edit: None,
        store: store.clone(),
        seen: Mutex::new(Vec::new()),
    });
    session(store.clone(), stub.clone())
        .evolve(
            "a-superseded-note-is-kept-out-of-the-bri",
            EvolveTrigger::Failure(run.id),
            None,
        )
        .await
        .expect("the pass completes");

    // Asking a model to reason from a claim already known to be wrong is worse
    // than telling it nothing.
    let prompt = &stub.seen.lock().unwrap()[0].instruction;
    assert!(!prompt.contains("the timeout is too short"), "{prompt}");
}

#[tokio::test]
async fn a_proposal_from_a_review_is_returned_verified_and_changes_nothing() {
    let (_home, store, run) = fixture("a-proposal-from-a-review-is-returned-ver");
    let before = store
        .get("a-proposal-from-a-review-is-returned-ver")
        .expect("readable")
        .expect("there");

    let proposing = Arc::new(StubReviewer {
        reply: "I suggest renaming it.".into(),
        edit: Some(Box::new(|store: &Arc<dyn WorkflowStore>| {
            let created_at = 5;
            store
                .save_proposal(&crate::workflows::WorkflowProposal {
                    id: mint_proposal_id(created_at),
                    workflow_id: "a-proposal-from-a-review-is-returned-ver".into(),
                    created_at,
                    rationale: "the name does not say what it does".into(),
                    ops: json!([{ "op": "set_node_name", "id": "work", "name": "Sweep" }]),
                    evidence_runs: vec!["run-1".into()],
                    note_ids: Vec::new(),
                    base_fingerprint: "unchecked".into(),
                    // Deliberately unverified: the pass has to check what it
                    // finds rather than trust that it was checked on the way in.
                    verification: None,
                    status: ProposalStatus::Pending,
                    decided_at: None,
                    decision_reason: None,
                })
                .expect("the proposal stores");
        })),
        store: store.clone(),
        seen: Mutex::new(Vec::new()),
    });

    let outcome = session(store.clone(), proposing)
        .evolve(
            "a-proposal-from-a-review-is-returned-ver",
            EvolveTrigger::Failure(run.id),
            None,
        )
        .await
        .expect("the pass completes");

    assert_eq!(outcome.proposals.len(), 1);
    assert!(
        outcome.proposals[0].verification.is_some(),
        "the pass verifies what it did not verify itself"
    );
    // The saved graph is untouched. This is the autonomy boundary.
    let after = store
        .get("a-proposal-from-a-review-is-returned-ver")
        .expect("readable")
        .expect("there");
    assert_eq!(after.graph, before.graph);
}

#[tokio::test]
async fn a_second_pass_is_refused_while_one_is_running() {
    let (_home, store, _run) = fixture("a-second-pass-is-refused-while-one-is-ru");
    let scope = store.proposal_decision_scope();
    let _guard = EvolveGuard::claim(&scope, "a-second-pass-is-refused-while-one-is-ru")
        .expect("the first claim wins");

    let outcome = session(store.clone(), silent(&store, "ok"))
        .evolve(
            "a-second-pass-is-refused-while-one-is-ru",
            EvolveTrigger::Manual,
            None,
        )
        .await
        .expect("a busy workflow is not an error");

    // A workflow failing ten times in a minute must not start ten harnesses.
    assert!(outcome.skipped);
    assert!(outcome.notes.is_empty());
    assert!(is_evolving(
        &scope,
        "a-second-pass-is-refused-while-one-is-ru"
    ));
}

#[tokio::test]
async fn the_same_workflow_id_in_another_store_can_evolve_concurrently() {
    let id = "the-same-workflow-id-in-another-store-can-evolve";
    let (_first_home, first_store, _first_run) = fixture(id);
    let (_second_home, second_store, _second_run) = fixture(id);
    let first_scope = first_store.proposal_decision_scope();
    let _guard = EvolveGuard::claim(&first_scope, id).expect("first store claims its workflow");

    let outcome = session(second_store.clone(), silent(&second_store, "ok"))
        .evolve(id, EvolveTrigger::Manual, None)
        .await
        .expect("the other store can review its own workflow");

    assert!(!outcome.skipped);
}

#[tokio::test]
async fn the_claim_is_released_when_the_pass_ends() {
    let id = "the-claim-is-released-when-the-pass-ends";
    let (_home, store, _run) = fixture(id);

    session(store.clone(), silent(&store, "ok"))
        .evolve(id, EvolveTrigger::Manual, None)
        .await
        .expect("the pass completes");

    // The release is a `Drop` on the guard, so the assertion only means
    // anything if a claim was actually taken — hence the `expect` above rather
    // than swallowing an error, and hence running against the workflow the
    // fixture installed rather than one that does not exist.
    assert!(
        !is_evolving(&store.proposal_decision_scope(), id),
        "a finished pass releases its claim"
    );

    // And a second pass can now run, which is the thing the release is for.
    let again = session(store.clone(), silent(&store, "ok"))
        .evolve(id, EvolveTrigger::Manual, None)
        .await
        .expect("the pass completes");
    assert!(!again.skipped);
}

#[tokio::test]
async fn a_review_of_an_unknown_workflow_fails_rather_than_inventing_one() {
    let (_home, store, _run) = fixture("a-review-of-an-unknown-workflow-fails-ra");

    let failed = session(store.clone(), silent(&store, "ok"))
        .evolve("no-such-workflow", EvolveTrigger::Manual, None)
        .await;

    assert!(failed.is_err());
    assert!(
        !is_evolving(&store.proposal_decision_scope(), "no-such-workflow"),
        "and claims nothing"
    );
}

#[tokio::test]
async fn a_disabled_host_refuses_the_pass() {
    let (_home, store, _run) = fixture("a-disabled-host-refuses-the-pass");
    let mut evolving = session(store.clone(), silent(&store, "ok"));
    evolving.config.enabled = false;

    let failed = evolving
        .evolve(
            "a-disabled-host-refuses-the-pass",
            EvolveTrigger::Manual,
            None,
        )
        .await;

    assert!(failed.is_err());
    // Nothing is written either: a switched-off feature does not half-run.
    assert!(store
        .list_notes("a-disabled-host-refuses-the-pass")
        .expect("readable")
        .is_empty());
}

#[tokio::test]
async fn a_trigger_naming_a_lost_run_still_reviews_the_rest() {
    let (_home, store, _run) = fixture("a-trigger-naming-a-lost-run-still-review");

    let outcome = session(store.clone(), silent(&store, "ok"))
        .evolve(
            "a-trigger-naming-a-lost-run-still-review",
            EvolveTrigger::Failure("run-gone".into()),
            None,
        )
        .await
        .expect("a missing run does not sink the review");

    // No system note — there was no run record to write one from — but the
    // journal and the rest of the history are still there to work with.
    assert!(outcome.notes.is_empty());
    assert_eq!(outcome.reply, "ok");
}

#[tokio::test]
async fn a_trigger_refuses_a_successful_run() {
    let (_home, store, mut run) = fixture("a-trigger-refuses-a-successful-run");
    run.status = RunStatus::Succeeded;
    run.error = None;
    store.record_run(&run).expect("updates the run");

    let err = session(store.clone(), silent(&store, "must not dispatch"))
        .evolve(
            "a-trigger-refuses-a-successful-run",
            EvolveTrigger::Failure(run.id),
            None,
        )
        .await
        .expect_err("only failures can trigger failure review");

    assert!(err.to_string().contains("not a failed run"), "got: {err}");
}

#[tokio::test]
async fn a_trigger_refuses_a_run_from_another_workflow() {
    let (_home, store, mut run) = fixture("a-trigger-refuses-a-foreign-run");
    run.workflow_id = "some-other-workflow".into();
    store.record_run(&run).expect("updates the run");

    let err = session(store.clone(), silent(&store, "must not dispatch"))
        .evolve(
            "a-trigger-refuses-a-foreign-run",
            EvolveTrigger::Failure(run.id),
            None,
        )
        .await
        .expect_err("a workflow cannot review another workflow's run");

    assert!(
        err.to_string().contains("belongs to workflow"),
        "got: {err}"
    );
}

#[tokio::test]
async fn verification_allows_findings_already_present_in_the_base_graph() {
    let (_home, store, _run) = fixture("verification-compares-with-the-base");
    ops::create(
        &store,
        &json!({
            "id": "verification-compares-with-the-base",
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
        "verification-compares-with-the-base",
    )
    .expect("installs input-dependent workflow");

    let proposed = ops::propose(
        &store,
        "verification-compares-with-the-base",
        "Clarify the step name without changing its bindings",
        &json!([{ "op": "set_node_name", "id": "say", "name": "Notify" }]),
        Vec::new(),
        Vec::new(),
    )
    .await
    .expect("proposal verifies");

    assert_eq!(proposed["ok"], json!(true), "{proposed}");
}

#[test]
fn the_config_is_switched_off_by_the_outer_workflows_switch() {
    let mut config = crate::config::WorkflowsConfig {
        enabled: false,
        ..Default::default()
    };
    assert!(
        !EvolveConfig::from_config(&config).enabled,
        "a host with workflows off has no runs to review"
    );

    config.enabled = true;
    config.evolve.enabled = false;
    assert!(!EvolveConfig::from_config(&config).enabled);

    config.evolve.enabled = true;
    assert!(EvolveConfig::from_config(&config).enabled);
}

#[tokio::test]
async fn a_review_turn_asks_for_the_restricted_tool_set() {
    let (_home, store, run) = fixture("a-review-turn-asks-for-restricted-tools");
    let stub = Arc::new(StubReviewer {
        reply: "ok".into(),
        edit: None,
        store: store.clone(),
        seen: Mutex::new(Vec::new()),
    });

    session(store.clone(), stub.clone())
        .evolve(
            "a-review-turn-asks-for-restricted-tools",
            EvolveTrigger::Failure(run.id),
            None,
        )
        .await
        .expect("the pass completes");

    // The whole autonomy argument rests on this reaching the harness. Asserted
    // on the dispatch rather than on `ToolMode` in isolation, because the mode
    // being *correct* and the mode being *sent* are different claims and only
    // the second one protects the graph.
    let seen = stub.seen.lock().unwrap();
    assert_eq!(
        seen[0].tool_mode.as_deref(),
        Some("propose:a-review-turn-asks-for-restricted-tools")
    );
}

#[test]
fn an_ordinary_copilot_turn_asks_for_nothing_and_gets_the_full_surface() {
    use crate::workflows::mcp::ToolMode;

    // The default has to stay permissive: every dispatch but a review's leaves
    // this unset, and a mistake here would silently disarm the copilot.
    assert_eq!(ToolMode::from_wire(None), ToolMode::Full);
    assert_eq!(ToolMode::from_wire(Some("propose")), ToolMode::Propose);
    assert_eq!(ToolMode::from_wire(Some("full")), ToolMode::Full);
    // Round-trips, so a frame written by one build is read the same by another.
    assert_eq!(
        ToolMode::from_wire(Some(ToolMode::Propose.as_wire())),
        ToolMode::Propose
    );
}
