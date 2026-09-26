//! Starting a workflow run without waiting for it, and reading it back.
//!
//! The failure this exists to prevent is the one measured in
//! tinyhumansai/medulla#209: a run that outlived its caller's idle ceiling was
//! reported to that caller as an abort, with no run id in the message, while it
//! carried on succeeding in the background. Recovering the result meant listing
//! a workflow's whole history and guessing which run was yours by start time.
//!
//! So this drives the real seam — `ops::run` onto a real embedded host — and
//! checks the three properties the fix rests on: the call comes back with the
//! run id before the run settles, the record is readable from that moment, and
//! waiting is still available for a caller that wants it.

#![cfg(feature = "workflows")]

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use medulla::workflows::local::LocalRun;
use medulla::workflows::ops::{self, StepDetail, Wait};
use medulla::workflows::{FileWorkflowStore, WorkflowStore};

/// An environment in which exactly `claude` looks installed.
///
/// Starting an embedded host requires a coding-agent CLI to exist, and these
/// workflows never dispatch to one — the binary is only ever detected, never
/// spawned. Pointing at the running test binary makes that true on every
/// platform without depending on what the machine happens to have.
fn env_with_only_claude() -> std::collections::HashMap<String, String> {
    std::collections::HashMap::from([
        ("PATH".to_string(), String::new()),
        (
            "TINYPLACE_CLAUDE_BIN".to_string(),
            std::env::current_exe()
                .expect("the test binary has a path")
                .to_string_lossy()
                .into_owned(),
        ),
    ])
}

/// A store on a scratch root, with its temporary directory kept alive.
fn store() -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    let root = tempfile::tempdir().expect("a temp dir");
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    (root, store)
}

/// A workflow of one `code` node, so a run exercises the whole host path
/// without needing a harness to answer.
fn save_a_runnable_workflow(store: &Arc<dyn WorkflowStore>, id: &str) {
    let document = json!({
        "id": id,
        "name": "Compute",
        "description": "adds two numbers",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "compute", "kind": "code", "name": "Compute",
              "config": { "language": "javascript", "source": "return { total: 1 + 1 };" } }
        ],
        "edges": [{ "from_node": "t", "to_node": "compute" }]
    })
    .to_string();
    ops::create(store, &document, id).expect("saves the workflow");
}

/// A workflow declaring one required input it never actually uses.
///
/// Its `code` node is unreachable by a call missing that input — the point is
/// never to get there, only to fail resolving inputs first.
fn save_a_workflow_requiring_an_input(store: &Arc<dyn WorkflowStore>, id: &str) {
    let document = json!({
        "id": id,
        "name": "Needs Input",
        "inputs": [{ "name": "repo", "type": "string", "required": true }],
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "compute", "kind": "code", "name": "Compute",
              "config": { "language": "javascript", "source": "return { total: 1 + 1 };" } }
        ],
        "edges": [{ "from_node": "t", "to_node": "compute" }]
    })
    .to_string();
    ops::create(store, &document, id).expect("saves the workflow");
}

/// The request these tests run, against a scratch workspace.
fn request<'a>(
    store: &Arc<dyn WorkflowStore>,
    config: &'a medulla::config::WorkflowsConfig,
    env: &'a std::collections::HashMap<String, String>,
    cwd: &'a std::path::Path,
    id: &'a str,
    launch: &'a medulla::harness_hooks::LaunchPolicy,
) -> LocalRun<'a> {
    LocalRun {
        store: store.clone(),
        config,
        custom_harnesses: &[],
        launch,
        env,
        cwd,
        workspace: None,
        workflow_id: id,
        input: tinyflows::engine::RunInput::new(json!({})),
        sink: None,
        liveness: None,
        origin: None,
    }
}

/// Poll `run_id` until it settles, or give up after `attempts` short waits.
async fn settled(
    store: &Arc<dyn WorkflowStore>,
    run_id: &str,
    attempts: usize,
) -> serde_json::Value {
    for _ in 0..attempts {
        let run = ops::get_run(store, run_id, StepDetail::Summary).expect("the run is readable");
        if run["status"] != "running" {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("run '{run_id}' never settled");
}

#[tokio::test]
async fn a_run_answers_with_its_id_before_it_finishes_and_finishes_anyway() {
    let (root, store) = store();
    save_a_runnable_workflow(&store, "compute");
    let config = medulla::config::WorkflowsConfig::default();
    let launch = medulla::harness_hooks::LaunchPolicy::default();
    let env = env_with_only_claude();

    let answer = ops::run(
        request(&store, &config, &env, root.path(), "compute", &launch),
        Wait::No,
    )
    .await
    .expect("the run is admitted");

    // The run id, in the answer, at the moment the run starts — the thing the
    // aborted caller in #209 never got and had to go find in the history.
    let run_id = answer["runId"].as_str().expect("a run id").to_string();
    assert_eq!(answer["status"], "running");
    assert_eq!(answer["workflowId"], "compute");
    assert!(!answer["run"].is_object(), "no record yet: {answer}");

    // And it is already a record, so the very next call can poll it rather than
    // failing with "no such run".
    let polled = ops::get_run(&store, &run_id, StepDetail::Summary).expect("readable at once");
    assert_eq!(polled["id"], json!(run_id));

    // The run did not need the call to stay open.
    let done = settled(&store, &run_id, 200).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(done["steps"][0]["nodeId"], "compute");
}

#[tokio::test]
async fn waiting_is_still_available_and_answers_with_the_whole_record() {
    let (root, store) = store();
    save_a_runnable_workflow(&store, "compute");
    let config = medulla::config::WorkflowsConfig::default();
    let launch = medulla::harness_hooks::LaunchPolicy::default();
    let env = env_with_only_claude();

    let answer = ops::run(
        request(&store, &config, &env, root.path(), "compute", &launch),
        Wait::Forever,
    )
    .await
    .expect("the run is admitted and awaited");

    // The pre-async shape, unchanged, for the short workflows where holding a
    // call open is honest.
    assert_eq!(answer["ok"], true);
    assert_eq!(answer["run"]["status"], "succeeded");
    assert_eq!(answer["run"]["steps"][0]["nodeId"], "compute");
}

#[tokio::test]
async fn a_wait_budget_that_expires_hands_back_the_run_rather_than_an_error() {
    let (root, store) = store();
    save_a_runnable_workflow(&store, "compute");
    let config = medulla::config::WorkflowsConfig::default();
    let launch = medulla::harness_hooks::LaunchPolicy::default();
    let env = env_with_only_claude();

    // A budget no run can beat. The point is the *shape* of what comes back
    // when it expires: a run to follow, not a failure to report.
    let answer = ops::run(
        request(&store, &config, &env, root.path(), "compute", &launch),
        Wait::Until(Duration::from_nanos(1)),
    )
    .await
    .expect("an expired budget is not an error");

    let run_id = answer["runId"].as_str().expect("a run id").to_string();
    assert_eq!(answer["status"], "running");
    assert_eq!(answer["waitedMs"], 0);
    let done = settled(&store, &run_id, 200).await;
    assert_eq!(done["status"], "succeeded", "{done}");
}

#[tokio::test]
async fn a_run_rejected_for_bad_inputs_is_never_admitted() {
    // Regression for the admission record `LocalRun::start` writes before the
    // detached task ever calls `run_workflow`: `run_workflow` resolves declared
    // inputs before it claims the run or writes its own record, so a caller
    // that omits a required one used to leave that admission record stuck at
    // `running` — nothing ever reconciled it, since the failure happened before
    // `RunFinalizer` (or anything else) existed for this run id.
    let (root, store) = store();
    save_a_workflow_requiring_an_input(&store, "needs-input");
    let config = medulla::config::WorkflowsConfig::default();
    let launch = medulla::harness_hooks::LaunchPolicy::default();
    let env = env_with_only_claude();

    let error = ops::run(
        request(&store, &config, &env, root.path(), "needs-input", &launch),
        Wait::No,
    )
    .await
    .expect_err("missing declared inputs are a synchronous call error");
    assert!(error.to_string().contains("repo"), "{error}");
    assert!(
        ops::list_runs(&store, "needs-input", StepDetail::Counts).unwrap()["runs"]
            .as_array()
            .unwrap()
            .is_empty(),
        "no invalid run should appear in history"
    );
}

#[tokio::test]
async fn a_disabled_workflow_is_refused_synchronously_rather_than_admitted() {
    let (root, store) = store();
    save_a_runnable_workflow(&store, "compute");
    let mut record = medulla::workflows::store::require(store.as_ref(), "compute").unwrap();
    record.enabled = false;
    store.save(&record).expect("disables the workflow");
    let config = medulla::config::WorkflowsConfig::default();
    let launch = medulla::harness_hooks::LaunchPolicy::default();
    let env = env_with_only_claude();

    let err = ops::run(
        request(&store, &config, &env, root.path(), "compute", &launch),
        Wait::No,
    )
    .await
    .expect_err("the operator's off switch");

    // The operator's own off switch, answered as an error rather than as a run
    // id that would never do anything. Async moved *waiting*, not refusing.
    assert!(err.to_string().contains("disabled"), "{err}");
    assert!(
        ops::list_runs(&store, "compute", StepDetail::Counts).unwrap()["runs"]
            .as_array()
            .unwrap()
            .is_empty(),
        "nothing should have been admitted"
    );
}
