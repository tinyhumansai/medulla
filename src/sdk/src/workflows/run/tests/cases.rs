//! Tests for running, pausing, resuming, and cancelling a workflow.
//!
//! Every run here goes through the real engine with the real capability seam;
//! only the harness dispatch is a stand-in, because the alternative is starting
//! a coding agent.

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
            usage: crate::tinyplace::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            harness: None,
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
                dispatch,
                resolver: Arc::new(StoreWorkflowResolver::new(self.store.clone())),
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
        Arc::new(StoreWorkflowResolver::new(harness.store.clone())),
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
            Default::default(),
        )
        .await
    });
    let second = tokio::spawn(async move {
        run_workflow(
            second_context,
            "gated",
            "run-b",
            json!({ "approvals": ["review"] }),
            Default::default(),
        )
        .await
    });
    wait_until_running("run-a").await;
    wait_until_running("run-b").await;

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

    let err = run_workflow(context, "diamond", "run-off", json!({}), Default::default())
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
            Default::default(),
        )
        .await
    });
    wait_until_running("dup").await;

    let second = run_workflow(
        second_context,
        "gated",
        "dup",
        json!({}),
        Default::default(),
    )
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
    let (guard, signal) = super::super::RunGuard::register("early");
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

// --- per-item fan-out ---

/// A dispatch that reports how many harness tasks overlapped, so a test can
/// tell a real fan-out from a sequential loop that merely finished.
#[derive(Default)]
pub(super) struct ConcurrencyProbe {
    live: std::sync::atomic::AtomicUsize,
    peak: std::sync::atomic::AtomicUsize,
    calls: std::sync::atomic::AtomicUsize,
}

impl ConcurrencyProbe {
    fn peak(&self) -> usize {
        self.peak.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl HarnessDispatch for ConcurrencyProbe {
    async fn dispatch(&self, _request: TaskRequest) -> Result<TaskOutcome, RunError> {
        use std::sync::atomic::Ordering::SeqCst;
        let live = self.live.fetch_add(1, SeqCst) + 1;
        self.peak.fetch_max(live, SeqCst);
        self.calls.fetch_add(1, SeqCst);
        // Stay "in flight" long enough for peers to start, so the gauge sees
        // real overlap rather than a lucky interleaving.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        self.live.fetch_sub(1, SeqCst);
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

/// `trigger → split_out → agent(per_item) → merge`: one harness task per
/// element of the trigger's `topics` array.
fn fanout(agent_config: serde_json::Value) -> String {
    let mut agent = json!({ "id": "work", "kind": "agent", "name": "Work" });
    agent["config"] = agent_config;
    json!({
        "id": "fanout",
        "name": "Fan out",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "Manual", "config": { "trigger_kind": "manual" } },
            { "id": "split", "kind": "split_out", "name": "Each", "config": { "path": "topics" } },
            agent,
            { "id": "join", "kind": "merge", "name": "Join" }
        ],
        "edges": [
            { "from_node": "t", "to_node": "split" },
            { "from_node": "split", "to_node": "work" },
            { "from_node": "work", "to_node": "join" }
        ]
    })
    .to_string()
}

/// Five topics for a fan-out to multiply over.
fn five_topics() -> serde_json::Value {
    json!({ "topics": [{ "n": 1 }, { "n": 2 }, { "n": 3 }, { "n": 4 }, { "n": 5 }] })
}

#[tokio::test]
async fn a_per_item_agent_node_dispatches_one_harness_task_per_item_concurrently() {
    let harness = Harness::new();
    harness.install(
        &fanout(json!({
            "prompt": "Handle the topic",
            "input_context": "=item",
            "execution": "per_item",
            "concurrency": 4,
        })),
        "fanout",
    );
    let dispatch = Arc::new(ConcurrencyProbe::default());

    let record = run_workflow(
        harness.context(dispatch.clone()),
        "fanout",
        "run-fanout",
        five_topics(),
        Default::default(),
    )
    .await
    .expect("runs");

    assert_eq!(record.status, RunStatus::Succeeded);
    assert_eq!(dispatch.calls(), 5, "one harness task per input item");
    assert!(
        dispatch.peak() > 1,
        "the point of the feature is overlap; peak was {}",
        dispatch.peak()
    );
}

#[tokio::test]
async fn the_run_ceiling_bounds_a_fan_out_wider_than_the_worker_pool() {
    // The graph asks for every item at once; the operator's ceiling is 2. The
    // run must be throttled to 2, not fail, and not honour the graph's request.
    let harness = Harness::new();
    let mut settings = (*harness.settings).clone();
    settings.max_parallel_agents = 2;

    harness.install(
        &fanout(json!({
            "prompt": "Handle the topic",
            "input_context": "=item",
            "execution": "per_item",
            "concurrency": "all",
        })),
        "fanout",
    );
    let dispatch = Arc::new(ConcurrencyProbe::default());
    let mut context = harness.context(dispatch.clone());
    context.settings = Arc::new(settings);

    let record = run_workflow(
        context,
        "fanout",
        "run-capped",
        five_topics(),
        Default::default(),
    )
    .await
    .expect("an over-wide fan-out is throttled, never rejected");

    assert_eq!(record.status, RunStatus::Succeeded);
    assert_eq!(dispatch.calls(), 5, "every item still ran");
    assert!(
        dispatch.peak() <= 2,
        "the host ceiling must win over the graph's request; peak was {}",
        dispatch.peak()
    );
}

#[tokio::test]
async fn without_concurrency_a_per_item_node_stays_sequential() {
    // Back-compat guard: a graph that does not ask for parallelism must not get
    // it, however many items it maps over.
    let harness = Harness::new();
    harness.install(
        &fanout(json!({
            "prompt": "Handle the topic",
            "input_context": "=item",
            "execution": "per_item",
        })),
        "fanout",
    );
    let dispatch = Arc::new(ConcurrencyProbe::default());

    run_workflow(
        harness.context(dispatch.clone()),
        "fanout",
        "run-seq",
        five_topics(),
        Default::default(),
    )
    .await
    .expect("runs");

    assert_eq!(dispatch.calls(), 5);
    assert_eq!(dispatch.peak(), 1, "unset concurrency stays sequential");
}

/// A workflow declaring a required `repo` and a defaulted `depth`, whose agent
/// prompt binds to both.
pub(super) fn parameterized() -> String {
    json!({
        "id": "parameterized",
        "name": "Parameterized",
        "inputs": [
            { "name": "repo", "type": "string", "required": true },
            { "name": "depth", "type": "number", "default": 3 }
        ],
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "=inputs.repo" } }
        ],
        "edges": [{ "from_node": "t", "to_node": "work" }]
    })
    .to_string()
}

/// Collect `pairs` into the supplied-values map a run takes.
fn values(pairs: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

#[tokio::test]
async fn a_declared_input_reaches_the_node_that_binds_to_it() {
    let harness = Harness::new();
    harness.install(&parameterized(), "parameterized");
    let dispatch = Arc::new(StubDispatch::default());

    let record = run_workflow(
        harness.context(dispatch.clone()),
        "parameterized",
        "run-inputs-1",
        json!({}),
        values(&[("repo", json!("acme/api"))]),
    )
    .await
    .expect("runs");

    assert_eq!(record.status, RunStatus::Succeeded);
    let instructions: Vec<String> = dispatch
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.instruction.clone())
        .collect();
    assert_eq!(
        instructions,
        vec!["acme/api".to_string()],
        "the agent's `=inputs.repo` prompt should have resolved to the supplied value"
    );
}

#[tokio::test]
async fn a_missing_required_input_records_no_run_at_all() {
    // The reason resolution happens before `RunGuard::claim` and `record_run`:
    // a rejected call must not leave a run in the operator's history.
    let harness = Harness::new();
    harness.install(&parameterized(), "parameterized");

    let err = run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "parameterized",
        "run-inputs-2",
        json!({}),
        Default::default(),
    )
    .await
    .expect_err("a missing required input must fail the call");
    assert!(
        err.to_string().contains("repo"),
        "the error should name the input: {err}"
    );

    assert!(
        harness.store.list_runs("parameterized").unwrap().is_empty(),
        "a rejected call must leave no run record behind"
    );
    assert!(!is_running("run-inputs-2"), "and must claim no run id");
}

#[tokio::test]
async fn a_wrongly_typed_input_is_refused_before_anything_dispatches() {
    let harness = Harness::new();
    harness.install(&parameterized(), "parameterized");
    let dispatch = Arc::new(StubDispatch::default());

    let err = run_workflow(
        harness.context(dispatch.clone()),
        "parameterized",
        "run-inputs-3",
        json!({}),
        values(&[("repo", json!("acme/api")), ("depth", json!("deep"))]),
    )
    .await
    .expect_err("a string for a number input must be refused");
    assert!(err.to_string().contains("depth"), "{err}");
    assert!(
        dispatch.seen.lock().unwrap().is_empty(),
        "nothing should have dispatched"
    );
}

#[tokio::test]
async fn a_dry_run_resolves_declared_inputs_too() {
    // A simulation exists to catch bindings that resolve to null, so it has to
    // see the same values a real run would.
    let harness = Harness::new();
    harness.install(&parameterized(), "parameterized");

    let result = dry_run(
        harness.store.clone(),
        Arc::new(StoreWorkflowResolver::new(harness.store.clone())),
        "parameterized",
        json!({}),
        values(&[("repo", json!("acme/api"))]),
    )
    .await
    .expect("simulates");

    assert_eq!(result.output["run"]["inputs"]["repo"], json!("acme/api"));
    assert_eq!(
        result.output["run"]["inputs"]["depth"],
        json!(3),
        "the declared default should have been applied"
    );
}
