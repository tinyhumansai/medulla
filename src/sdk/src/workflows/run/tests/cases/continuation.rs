//! Concurrency, policy, retry, and persistence cases for workflow runs.

use super::*;

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
    let (guard, signal) = super::super::super::RunGuard::register("early");
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

    let resolver = StoreWorkflowResolver::new(harness.store.clone(), u64::MAX);
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
            usage: crate::protocol::TokenUsage {
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
        Arc::new(StoreWorkflowResolver::new(harness.store.clone(), u64::MAX)),
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
