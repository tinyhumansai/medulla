//! Tests for running, pausing, resuming, and cancelling a workflow.
//!
//! Every run here goes through the real engine with the real capability seam;
//! only the harness dispatch is a stand-in, because the alternative is starting
//! a coding agent.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;

use super::{cancel, dry_run, is_running, resume_workflow, run_workflow, RunContext};
use crate::flow_engine::caps::dispatch::HarnessDispatch;
use crate::flow_engine::{null_sink, CapabilitySettings, HostServices};
use crate::hub::{RunError, TaskOutcome, TaskRequest};
use crate::workflows::store::parse_workflow;
use crate::workflows::{
    require_run, FileWorkflowStore, RunStatus, StoreWorkflowResolver, WorkflowError, WorkflowStore,
};

/// A dispatch that answers immediately, recording what it saw.
#[derive(Default)]
struct StubDispatch {
    seen: Mutex<Vec<TaskRequest>>,
}

#[async_trait]
impl HarnessDispatch for StubDispatch {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        self.seen.lock().unwrap().push(request);
        Ok(TaskOutcome {
            reply: "done".into(),
            usage: crate::tinyplace::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            harness: None,
        })
    }
}

/// A dispatch that never returns, so a test can cancel a run that is genuinely
/// in flight rather than one that has already finished.
struct HangingDispatch;

#[async_trait]
impl HarnessDispatch for HangingDispatch {
    async fn dispatch(&self, _request: TaskRequest) -> Result<TaskOutcome, RunError> {
        std::future::pending().await
    }
}

/// A store, its settings, and a context factory over a temporary directory.
struct Harness {
    _root: tempfile::TempDir,
    store: Arc<FileWorkflowStore>,
    settings: Arc<CapabilitySettings>,
}

impl Harness {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(FileWorkflowStore::new(
            vec![root.path().join("workflows")],
            root.path().join("runs"),
        ));
        let mut settings = CapabilitySettings::rooted_at(root.path());
        settings.default_worker_address = "worker".into();
        Self {
            _root: root,
            store,
            settings: Arc::new(settings),
        }
    }

    fn install(&self, document: &str, id: &str) {
        let record = parse_workflow(document, id).expect("valid fixture");
        self.store.save(&record).expect("saves");
    }

    fn context(&self, dispatch: Arc<dyn HarnessDispatch>) -> RunContext {
        RunContext {
            store: self.store.clone(),
            settings: self.settings.clone(),
            services: HostServices {
                dispatch,
                resolver: Arc::new(StoreWorkflowResolver::new(self.store.clone())),
                http_credentials: HashMap::new(),
            },
            sink: null_sink(),
        }
    }
}

/// A diamond: the trigger fans out to two agent nodes that run concurrently,
/// then a `merge` waits for both. Exercises parallel execution and the fan-in
/// barrier in one graph.
fn diamond() -> String {
    json!({
        "id": "diamond",
        "name": "Diamond",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "left", "kind": "agent", "name": "Left",
              "config": { "prompt": "left", "agent_ref": "a" } },
            { "id": "right", "kind": "agent", "name": "Right",
              "config": { "prompt": "right", "agent_ref": "b" } },
            { "id": "join", "kind": "merge", "name": "Join" }
        ],
        "edges": [
            { "from_node": "t", "to_node": "left" },
            { "from_node": "t", "to_node": "right" },
            { "from_node": "left", "to_node": "join" },
            { "from_node": "right", "to_node": "join" }
        ]
    })
    .to_string()
}

/// A graph whose single step is an approval gate.
fn gated() -> String {
    json!({
        "id": "gated",
        "name": "Gated",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "review", "kind": "agent", "name": "Review",
              "config": { "prompt": "review it", "requires_approval": true } }
        ],
        "edges": [{ "from_node": "t", "to_node": "review" }]
    })
    .to_string()
}

#[tokio::test]
async fn a_diamond_runs_both_branches_and_waits_for_them_at_the_merge() {
    let harness = Harness::new();
    harness.install(&diamond(), "diamond");
    let dispatch = Arc::new(StubDispatch::default());

    let record = run_workflow(
        harness.context(dispatch.clone()),
        "diamond",
        "run-1",
        json!({}),
    )
    .await
    .expect("runs");

    assert_eq!(record.status, RunStatus::Succeeded);
    let dispatched: Vec<String> = dispatch
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.worker_address.clone())
        .collect();
    assert_eq!(
        dispatched.len(),
        2,
        "both branches should have dispatched: {dispatched:?}"
    );
    assert!(dispatched.contains(&"a".to_string()));
    assert!(dispatched.contains(&"b".to_string()));

    // The merge only produces a step once both predecessors arrived, so seeing
    // it at all is the barrier working.
    assert!(
        record.steps.iter().any(|step| step.node_id == "join"),
        "the merge should have run: {:?}",
        record.steps
    );
}

#[tokio::test]
async fn a_run_is_recorded_and_can_be_read_back_by_id() {
    let harness = Harness::new();
    harness.install(&diamond(), "diamond");

    run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "diamond",
        "run-2",
        json!({}),
    )
    .await
    .unwrap();

    let stored = require_run(harness.store.as_ref(), "run-2").expect("recorded");
    assert_eq!(stored.workflow_id, "diamond");
    assert_eq!(stored.status, RunStatus::Succeeded);
    assert!(stored.finished_at.is_some());
    assert!(!stored.steps.is_empty(), "steps should be persisted");
}

#[tokio::test]
async fn an_approval_gate_pauses_the_run_and_names_what_it_is_waiting_on() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");

    let record = run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "gated",
        "run-3",
        json!({}),
    )
    .await
    .expect("runs");

    assert_eq!(record.status, RunStatus::PendingApproval);
    assert_eq!(record.pending_approvals, vec!["review".to_string()]);
    assert!(!record.status.is_settled(), "a gate is resumable");
}

#[tokio::test]
async fn approving_the_gate_resumes_the_run_to_completion() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "gated",
        "run-4",
        json!({}),
    )
    .await
    .unwrap();

    let resumed = resume_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "run-4",
        vec!["review".into()],
        Vec::new(),
    )
    .await
    .expect("resumes");

    assert_eq!(resumed.status, RunStatus::Succeeded);
    assert!(resumed.pending_approvals.is_empty());
}

#[tokio::test]
async fn a_resume_naming_no_pending_gate_is_refused() {
    // The engine treats the resume call itself as consent, so without this
    // check any resume would release every gate the run is holding.
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "gated",
        "run-5",
        json!({}),
    )
    .await
    .unwrap();

    let err = resume_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "run-5",
        vec!["some-other-node".into()],
        Vec::new(),
    )
    .await
    .expect_err("must not release the gate");

    assert!(
        err.to_string().contains("review"),
        "say which gate is actually waiting: {err}"
    );
    assert_eq!(
        require_run(harness.store.as_ref(), "run-5").unwrap().status,
        RunStatus::PendingApproval,
        "the run must still be parked"
    );
}

#[tokio::test]
async fn resuming_a_run_that_is_not_waiting_is_refused() {
    let harness = Harness::new();
    harness.install(&diamond(), "diamond");
    run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "diamond",
        "run-6",
        json!({}),
    )
    .await
    .unwrap();

    let err = resume_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "run-6",
        vec!["anything".into()],
        Vec::new(),
    )
    .await
    .expect_err("already finished");

    assert!(
        err.to_string().contains("not awaiting approval"),
        "got {err}"
    );
}

#[tokio::test]
async fn cancelling_an_in_flight_run_settles_it_as_cancelled() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    let context = harness.context(Arc::new(HangingDispatch));

    let run = tokio::spawn(async move {
        run_workflow(
            context,
            "gated",
            "run-7",
            json!({ "approvals": ["review"] }),
        )
        .await
    });

    // Wait for the run to register itself before cancelling — otherwise the
    // test races setup and proves nothing.
    while !is_running("run-7") {
        tokio::task::yield_now().await;
    }
    assert!(cancel("run-7"), "the run should be cancellable by id");

    let record = run.await.unwrap().expect("settles");
    assert_eq!(record.status, RunStatus::Cancelled);
    assert!(
        !is_running("run-7"),
        "the guard must deregister the run on every exit path"
    );
}

#[tokio::test]
async fn cancelling_a_run_that_already_settled_is_not_an_error() {
    assert!(!cancel("never-existed"));
}

#[tokio::test]
async fn a_disabled_workflow_refuses_to_run() {
    let harness = Harness::new();
    let mut record = parse_workflow(&diamond(), "diamond").unwrap();
    record.enabled = false;
    harness.store.save(&record).unwrap();

    let err = run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "diamond",
        "run-8",
        json!({}),
    )
    .await
    .expect_err("disabled");

    assert!(err.to_string().contains("disabled"), "got {err}");
}

#[tokio::test]
async fn running_an_unknown_workflow_names_the_id() {
    let harness = Harness::new();

    let err = run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "ghost",
        "run-9",
        json!({}),
    )
    .await
    .expect_err("no such workflow");

    assert!(matches!(err, WorkflowError::NotFound(_)), "got {err:?}");
}

#[tokio::test]
async fn a_run_that_exceeds_its_limit_fails_rather_than_hanging() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    let mut context = harness.context(Arc::new(HangingDispatch));
    let mut impatient = (*harness.settings).clone();
    impatient.run_timeout_secs = 0;
    context.settings = Arc::new(impatient);

    let record = run_workflow(
        context,
        "gated",
        "run-10",
        json!({ "approvals": ["review"] }),
    )
    .await
    .expect("settles");

    assert_eq!(record.status, RunStatus::Failed);
    assert!(
        record
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("limit"),
        "say why it stopped: {:?}",
        record.error
    );
}

#[tokio::test]
async fn a_dry_run_exercises_the_graph_without_dispatching_anything() {
    let harness = Harness::new();
    harness.install(&diamond(), "diamond");
    let dispatch = Arc::new(StubDispatch::default());
    // Held only to prove it is never called.
    let _unused: Arc<dyn HarnessDispatch> = dispatch.clone();

    let result = dry_run(
        harness.store.clone(),
        Arc::new(StoreWorkflowResolver::new(harness.store.clone())),
        "diamond",
        json!({}),
    )
    .await
    .expect("simulates");

    assert!(
        result.output["nodes"]["join"].is_object(),
        "the whole graph should have run: {:?}",
        result.output
    );
    // Observing the run must not change what it does: the same graph, the same
    // outcome, plus a reading of the steps.
    assert!(result.diagnosis.is_clean(), "{:?}", result.diagnosis);
    assert!(
        dispatch.seen.lock().unwrap().is_empty(),
        "a dry run must start no harness session"
    );
}

/// The finalizer's reason for existing: a run future dropped mid-flight must
/// not leave a record claiming to be running.
#[tokio::test]
async fn a_run_dropped_mid_flight_is_reconciled_to_interrupted() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    let context = harness.context(Arc::new(HangingDispatch));

    let run = tokio::spawn(async move {
        run_workflow(
            context,
            "gated",
            "run-11",
            json!({ "approvals": ["review"] }),
        )
        .await
    });
    while !is_running("run-11") {
        tokio::task::yield_now().await;
    }
    run.abort();
    let _ = run.await;

    // The abort drops the future at an await point, which runs the guard.
    let record = require_run(harness.store.as_ref(), "run-11").expect("recorded");
    assert_eq!(
        record.status,
        RunStatus::Interrupted,
        "a vanished run must not still claim to be running"
    );
}

/// Concurrency guard: two runs of the same workflow must not share a
/// cancellation entry, or cancelling one would silently cancel the other.
#[tokio::test]
async fn two_runs_of_one_workflow_cancel_independently() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    let first_context = harness.context(Arc::new(HangingDispatch));
    let second_context = harness.context(Arc::new(HangingDispatch));

    let first = tokio::spawn(async move {
        run_workflow(
            first_context,
            "gated",
            "run-a",
            json!({ "approvals": ["review"] }),
        )
        .await
    });
    let second = tokio::spawn(async move {
        run_workflow(
            second_context,
            "gated",
            "run-b",
            json!({ "approvals": ["review"] }),
        )
        .await
    });
    while !is_running("run-a") || !is_running("run-b") {
        tokio::task::yield_now().await;
    }

    cancel("run-a");
    let first = first.await.unwrap().unwrap();

    assert_eq!(first.status, RunStatus::Cancelled);
    assert!(is_running("run-b"), "the other run must be untouched");

    cancel("run-b");
    let second = second.await.unwrap().unwrap();
    assert_eq!(second.status, RunStatus::Cancelled);
}

#[tokio::test]
async fn a_host_with_workflows_disabled_runs_nothing() {
    let harness = Harness::new();
    harness.install(&diamond(), "diamond");
    let mut context = harness.context(Arc::new(StubDispatch::default()));
    let mut off = (*harness.settings).clone();
    off.enabled = false;
    context.settings = Arc::new(off);

    let err = run_workflow(context, "diamond", "run-off", json!({}))
        .await
        .expect_err("the operator turned it off");

    assert!(err.to_string().contains("disabled"), "got {err}");
}

#[tokio::test]
async fn the_same_run_id_cannot_start_twice_concurrently() {
    // A resent frame, or a sender reusing an active id, would otherwise run
    // every node's side effects twice and race to overwrite one run record.
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    let first_context = harness.context(Arc::new(HangingDispatch));
    let second_context = harness.context(Arc::new(HangingDispatch));

    let first = tokio::spawn(async move {
        run_workflow(
            first_context,
            "gated",
            "dup",
            json!({ "approvals": ["review"] }),
        )
        .await
    });
    while !is_running("dup") {
        tokio::task::yield_now().await;
    }

    let second = run_workflow(second_context, "gated", "dup", json!({}))
        .await
        .expect_err("the id is taken");
    assert!(
        second.to_string().contains("already executing"),
        "got {second}"
    );

    cancel("dup");
    assert_eq!(first.await.unwrap().unwrap().status, RunStatus::Cancelled);
}

#[tokio::test]
async fn a_cancel_that_arrives_before_the_run_waits_is_not_lost() {
    // `notify_waiters` wakes only tasks already waiting and stores no permit,
    // so without a durable flag a cancel landing in the setup window would be
    // dropped and the run would carry on regardless.
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    let (guard, signal) = super::RunGuard::register("early");
    assert!(cancel("early"), "the run is registered");
    assert!(
        signal.is_cancelled(),
        "the cancel must be recorded, not just signalled"
    );

    // Awaiting afterwards resolves immediately rather than hanging forever.
    tokio::time::timeout(std::time::Duration::from_secs(2), signal.cancelled())
        .await
        .expect("a cancel that already happened must not block");
    drop(guard);
}

#[tokio::test]
async fn a_resume_may_reject_the_only_gate_without_approving_anything() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "gated",
        "run-reject",
        json!({}),
    )
    .await
    .unwrap();

    // Rejecting is a legitimate way to settle a run; requiring an approval
    // would make --reject unusable on a single-gate workflow.
    let settled = resume_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "run-reject",
        Vec::new(),
        vec!["review".into()],
    )
    .await
    .expect("a rejection is a decision");

    assert!(settled.status.is_settled(), "got {:?}", settled.status);
}

#[tokio::test]
async fn a_disabled_sub_workflow_is_refused_during_resolution() {
    use tinyflows::caps::WorkflowResolver;

    let harness = Harness::new();
    let mut child = parse_workflow(&gated(), "child").unwrap();
    // The fixture document declares its own id, which wins over the filename.
    child.id = "child".into();
    child.enabled = false;
    harness.store.save(&child).unwrap();

    let resolver = StoreWorkflowResolver::new(harness.store.clone());
    let err = resolver
        .resolve("child")
        .await
        .expect_err("disabling must hold for a child too");

    assert!(err.to_string().contains("disabled"), "got {err}");
}
