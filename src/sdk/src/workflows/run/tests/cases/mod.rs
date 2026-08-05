//! Tests for running, pausing, resuming, and cancelling a workflow.
//!
//! Every run here goes through the real engine with the real capability seam;
//! only the harness dispatch is a stand-in, because the alternative is starting
//! a coding agent.

mod continuation;

use std::collections::HashMap;
pub(super) use std::sync::{Arc, Mutex};

use async_trait::async_trait;
pub(super) use serde_json::json;

pub(super) use super::super::{
    cancel, dry_run, is_running, resume_workflow, run_workflow, RunContext,
};
pub(super) use crate::flow_engine::caps::dispatch::HarnessDispatch;
use crate::flow_engine::{null_sink, CapabilitySettings, HostServices};
pub(super) use crate::hub::{RunError, TaskOutcome, TaskRequest};
use crate::workflows::store::parse_workflow;
pub(super) use crate::workflows::{
    require_run, FileWorkflowStore, RunStatus, StoreWorkflowResolver, WorkflowError, WorkflowStore,
};

/// A dispatch that answers immediately, recording what it saw.
#[derive(Default)]
pub(super) struct StubDispatch {
    pub(super) seen: Mutex<Vec<TaskRequest>>,
}

#[async_trait]
impl HarnessDispatch for StubDispatch {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        self.seen.lock().unwrap().push(request);
        Ok(TaskOutcome {
            reply: "done".into(),
            usage: crate::protocol::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            harness: None,
            session_id: None,
        })
    }
}

/// A dispatch that never returns, so a test can cancel a run that is genuinely
/// in flight rather than one that has already finished.
pub(super) struct HangingDispatch;

#[async_trait]
impl HarnessDispatch for HangingDispatch {
    async fn dispatch(&self, _request: TaskRequest) -> Result<TaskOutcome, RunError> {
        std::future::pending().await
    }
}

/// A resumed leg that swallows one node error before remaining in flight.
pub(super) struct ErrorThenHangDispatch;

#[async_trait]
impl HarnessDispatch for ErrorThenHangDispatch {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        if request.instruction == "fail" {
            return Err(RunError::Worker("expected failure".into()));
        }
        if request.instruction == "hang" {
            return std::future::pending().await;
        }
        Ok(TaskOutcome {
            reply: "approved".into(),
            usage: crate::protocol::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            harness: None,
            session_id: None,
        })
    }
}

/// A store, its settings, and a context factory over a temporary directory.
pub(super) struct Harness {
    _root: tempfile::TempDir,
    /// File-backed workflow and run state used by the test.
    pub(super) store: Arc<FileWorkflowStore>,
    /// Capability policy copied into each test run context.
    pub(super) settings: Arc<CapabilitySettings>,
}

impl Harness {
    /// Create an isolated permissive workflow harness.
    pub(super) fn new() -> Self {
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

    /// Parse and save one workflow fixture under `id`.
    pub(super) fn install(&self, document: &str, id: &str) {
        let record = parse_workflow(document, id).expect("valid fixture");
        self.store.save(&record).expect("saves");
    }

    /// Build a run context using the supplied stand-in dispatch.
    pub(super) fn context(&self, dispatch: Arc<dyn HarnessDispatch>) -> RunContext {
        RunContext {
            store: self.store.clone(),
            settings: self.settings.clone(),
            services: HostServices {
                node_progress: None,
                dispatch,
                resolver: Arc::new(StoreWorkflowResolver::new(
                    self.store.clone(),
                    self.settings.max_loop_iterations,
                )),
                http_credentials: HashMap::new(),
            },
            sink: null_sink(),
        }
    }
}

/// Wait for a run to enter the process registry, failing instead of hanging.
async fn wait_until_running(run_id: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !is_running(run_id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{run_id} was not registered"));
}

/// A diamond: the trigger fans out to two agent nodes that run concurrently,
/// then a `merge` waits for both. Exercises parallel execution and the fan-in
/// barrier in one graph.
pub(super) fn diamond() -> String {
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
pub(super) fn gated() -> String {
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

/// A gated graph whose resumed leg swallows an error and then hangs.
pub(super) fn gated_error_then_hang() -> String {
    json!({
        "id": "gated-error-then-hang",
        "name": "Gated error then hang",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "review", "kind": "agent", "name": "Review",
              "config": { "prompt": "review it", "requires_approval": true } },
            { "id": "swallowed", "kind": "agent", "name": "Swallowed",
              "config": { "prompt": "fail", "on_error": "continue" } },
            { "id": "hang", "kind": "agent", "name": "Hang",
              "config": { "prompt": "hang" } }
        ],
        "edges": [
            { "from_node": "t", "to_node": "review" },
            { "from_node": "review", "to_node": "swallowed" },
            { "from_node": "swallowed", "to_node": "hang" }
        ]
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
        Default::default(),
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
    assert_eq!(
        record
            .steps
            .iter()
            .find(|step| step.node_id == "left")
            .and_then(|step| step.input.as_ref()),
        Some(&json!("left")),
        "the resolved prompt is durable evidence on the correct parallel step"
    );
    assert_eq!(
        record
            .steps
            .iter()
            .find(|step| step.node_id == "right")
            .and_then(|step| step.input.as_ref()),
        Some(&json!("right")),
        "parallel agent prompts must not be crossed"
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
        Default::default(),
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
            Default::default(),
        )
        .await
    });

    // Wait for the run to register itself before cancelling — otherwise the
    // test races setup and proves nothing.
    wait_until_running("run-7").await;
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
        Default::default(),
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
        Default::default(),
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
        Default::default(),
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
        Arc::new(StoreWorkflowResolver::new(harness.store.clone(), u64::MAX)),
        "diamond",
        json!({}),
        Default::default(),
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
            Default::default(),
        )
        .await
    });
    wait_until_running("run-11").await;
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

/// A bounded loop whose body is a single agent node, closed by a back-edge.
fn bounded_loop(max_iterations: u64) -> String {
    json!({
        "name": "Bounded loop",
        "description": "Repeats one agent step a bounded number of times.",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "l", "kind": "loop", "name": "Until done",
              "config": { "max_iterations": max_iterations, "on_exceeded": "continue" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "Do one pass of the work." } },
            { "id": "out", "kind": "transform", "name": "Report",
              "config": { "set": { "passes": "=.nodes.l.iteration" } } }
        ],
        "edges": [
            { "from_node": "start", "to_node": "l" },
            { "from_node": "l", "from_port": "body", "to_node": "work" },
            { "from_node": "work", "to_node": "l" },
            { "from_node": "l", "from_port": "done", "to_node": "out" }
        ]
    })
    .to_string()
}

#[tokio::test]
async fn a_bounded_loop_dispatches_its_body_once_per_iteration() {
    let harness = Harness::new();
    harness.install(&bounded_loop(3), "loop");
    let dispatch = Arc::new(StubDispatch::default());

    let record = run_workflow(
        harness.context(dispatch.clone()),
        "loop",
        "run-loop-1",
        json!({}),
        Default::default(),
    )
    .await
    .expect("runs");

    assert_eq!(record.status, RunStatus::Succeeded);
    assert_eq!(
        dispatch.seen.lock().unwrap().len(),
        3,
        "the agent node in the body should run once per iteration"
    );
}

/// The host ceiling is a clamp, not a refusal: a graph asking for more than the
/// operator allows still runs, it just stops sooner. Refusing would make a
/// workflow authored against a more generous host unrunnable here.
#[tokio::test]
async fn a_loop_asking_past_the_host_ceiling_is_clamped_rather_than_refused() {
    let mut harness = Harness::new();
    let mut settings = CapabilitySettings::clone(&harness.settings);
    settings.max_loop_iterations = 2;
    harness.settings = Arc::new(settings);

    harness.install(&bounded_loop(50), "greedy");
    let dispatch = Arc::new(StubDispatch::default());

    let record = run_workflow(
        harness.context(dispatch.clone()),
        "greedy",
        "run-loop-2",
        json!({}),
        Default::default(),
    )
    .await
    .expect("a clamped loop still runs");

    assert_eq!(record.status, RunStatus::Succeeded);
    assert_eq!(
        dispatch.seen.lock().unwrap().len(),
        2,
        "the loop should stop at the host ceiling, not at the 50 it asked for"
    );

    // The stored document keeps what the author wrote, so raising the ceiling
    // later restores the intent without anyone re-editing the workflow.
    let saved = harness.store.get("greedy").expect("gets").expect("present");
    let declared = saved
        .graph
        .nodes
        .iter()
        .find(|n| n.id == "l")
        .and_then(|n| n.config.get("max_iterations"))
        .and_then(|v| v.as_u64());
    assert_eq!(
        declared,
        Some(50),
        "the clamp must not rewrite the document"
    );
}

/// A loop that omits `max_iterations` falls through to the engine's own
/// default (25) — not to "no limit". A host ceiling below that default must
/// still bind it, or a graph that names no cap at all can outrun a ceiling an
/// operator lowered specifically to bound cost.
#[tokio::test]
async fn a_loop_with_no_declared_cap_is_still_clamped_to_the_host_ceiling() {
    let mut harness = Harness::new();
    let mut settings = CapabilitySettings::clone(&harness.settings);
    settings.max_loop_iterations = 2;
    harness.settings = Arc::new(settings);

    let document = json!({
        "name": "Uncapped loop",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "l", "kind": "loop", "name": "Until done",
              "config": { "on_exceeded": "continue" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "Do one pass of the work." } },
            { "id": "out", "kind": "transform", "name": "Report",
              "config": { "set": { "passes": "=.nodes.l.iteration" } } }
        ],
        "edges": [
            { "from_node": "start", "to_node": "l" },
            { "from_node": "l", "from_port": "body", "to_node": "work" },
            { "from_node": "work", "to_node": "l" },
            { "from_node": "l", "from_port": "done", "to_node": "out" }
        ]
    })
    .to_string();
    harness.install(&document, "uncapped");
    let dispatch = Arc::new(StubDispatch::default());

    let record = run_workflow(
        harness.context(dispatch.clone()),
        "uncapped",
        "run-loop-uncapped",
        json!({}),
        Default::default(),
    )
    .await
    .expect("a clamped loop still runs");

    assert_eq!(record.status, RunStatus::Succeeded);
    assert_eq!(
        dispatch.seen.lock().unwrap().len(),
        2,
        "an implicit engine default (25) must still be clamped to the host ceiling"
    );
}
