//! Pointing a run at a checkout the caller is not standing in.
//!
//! `workspace` is a run parameter rather than something a workflow declares,
//! and this drives the seam that makes that true: a `medulla:shell` step runs
//! in the named directory, resolves its own `args.cwd` and `$MEDULLA_WORKSPACE`
//! against it, and a directory that is not there is refused before anything
//! spawns.
//!
//! The failure it exists to prevent is the one that made the parameter
//! necessary: a workflow aimed at another repository had to carry the path as a
//! declared input, and the script policy then refused it for being absolute or
//! for traversing out of the workspace — so the workflow could name the other
//! checkout but never actually run in it.

#![cfg(all(feature = "workflows", unix))]

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use medulla::workflows::local::LocalRun;
use medulla::workflows::ops::{self, StepDetail, Wait};
use medulla::workflows::{FileWorkflowStore, WorkflowStore};

/// An environment in which exactly `claude` looks installed, and `HOME` points
/// at a scratch directory.
///
/// Starting an embedded host requires a coding-agent CLI to exist; these
/// workflows never dispatch to one, so pointing at the running test binary makes
/// that true without depending on the machine.
fn env(home: &std::path::Path) -> std::collections::HashMap<String, String> {
    std::collections::HashMap::from([
        // Not empty: a `medulla:shell` step spawns a real interpreter, which has
        // to be findable.
        (
            "PATH".to_string(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
        ),
        ("HOME".to_string(), home.to_string_lossy().into_owned()),
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

/// A workflow of one shell step that reports where it ran and what the run's
/// workspace variable says.
fn save_a_reporting_workflow(store: &Arc<dyn WorkflowStore>, id: &str) {
    let document = json!({
        "id": id,
        "name": "Report the workspace",
        "description": "prints the directory the step ran in",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "where", "kind": "tool_call", "name": "Where am I",
              "config": {
                  "slug": "medulla:shell",
                  "args": {
                      "language": "bash",
                      "script": "printf '{\"marker\":\"%s\",\"declared\":\"%s\"}' \
                                 \"$(cat marker.txt)\" \"$MEDULLA_WORKSPACE\""
                  }
              } }
        ],
        "edges": [{ "from_node": "t", "to_node": "where" }]
    })
    .to_string();
    ops::create(store, &document, id).expect("saves the workflow");
}

/// A checkout stand-in: a directory holding a marker file naming itself.
fn checkout(marker: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("marker.txt"), marker).expect("the marker file");
    dir
}

/// Poll `run_id` until it settles, or give up after `attempts` short waits.
async fn settled(
    store: &Arc<dyn WorkflowStore>,
    run_id: &str,
    attempts: usize,
) -> serde_json::Value {
    for _ in 0..attempts {
        let run = ops::get_run(store, run_id, StepDetail::Full).expect("the run is readable");
        if run["status"] != "running" {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("run '{run_id}' never settled");
}

/// The case the parameter exists for: the run works in a directory that is
/// nowhere under the one the caller is sitting in.
#[tokio::test]
async fn a_named_workspace_is_where_the_run_actually_works() {
    let (root, store) = store();
    save_a_reporting_workflow(&store, "report");
    let elsewhere = checkout("the other checkout");
    let config = medulla::config::WorkflowsConfig::default();
    let launch = medulla::harness_hooks::LaunchPolicy::default();
    let env = env(root.path());

    let value = ops::run(
        LocalRun {
            store: store.clone(),
            config: &config,
            custom_harnesses: &[],
            launch: &launch,
            env: &env,
            // The session's own directory holds no marker at all, so a step that
            // ignored the parameter fails rather than passing by accident.
            cwd: root.path(),
            workspace: Some(elsewhere.path().to_string_lossy().into_owned()),
            workflow_id: "report",
            input: tinyflows::engine::RunInput::new(json!({})),
            sink: None,
            liveness: None,
            origin: Some(medulla::workflows::RunOrigin::of_kind(
                medulla::workflows::RunOrigin::CLI,
            )),
        },
        Wait::No,
    )
    .await
    .expect("the run is admitted");

    let run_id = value["runId"].as_str().expect("a run id").to_string();
    let run = settled(&store, &run_id, 200).await;
    assert_eq!(run["status"], "succeeded", "run: {run}");

    let output = run["steps"]
        .as_array()
        .and_then(|steps| steps.iter().find(|step| step["nodeId"] == "where"))
        .map(|step| step["output"].clone())
        .unwrap_or_else(|| panic!("the shell step's output; run: {run}"));
    // The step's own `{ output, stderr }`, under the engine's item envelope.
    let reported = &output[0]["json"]["json"]["output"];
    assert_eq!(
        reported["marker"], "the other checkout",
        "the step ran outside the named workspace: {output}"
    );
    let declared = reported["declared"].as_str().expect("a workspace path");
    assert_eq!(
        std::fs::canonicalize(declared).expect("a real path"),
        std::fs::canonicalize(elsewhere.path()).expect("a real path"),
        "$MEDULLA_WORKSPACE should name the run's workspace: {output}"
    );

    // And the record says which checkout it touched, which is the only way to
    // tell two runs of the same workflow apart afterwards.
    let workspace = run["origin"]["workspace"].as_str().expect("a workspace");
    assert_eq!(
        std::fs::canonicalize(workspace).expect("a real path"),
        std::fs::canonicalize(elsewhere.path()).expect("a real path")
    );
}

/// A mistyped checkout is refused by the call, not discovered halfway through a
/// run that quietly went to work on the caller's own directory.
#[tokio::test]
async fn a_workspace_that_is_not_there_refuses_the_run() {
    let (root, store) = store();
    save_a_reporting_workflow(&store, "report");
    let config = medulla::config::WorkflowsConfig::default();
    let launch = medulla::harness_hooks::LaunchPolicy::default();
    let env = env(root.path());

    let error = ops::run(
        LocalRun {
            store: store.clone(),
            config: &config,
            custom_harnesses: &[],
            launch: &launch,
            env: &env,
            cwd: root.path(),
            workspace: Some("/no/such/checkout".to_string()),
            workflow_id: "report",
            input: tinyflows::engine::RunInput::new(json!({})),
            sink: None,
            liveness: None,
            origin: None,
        },
        Wait::No,
    )
    .await
    .expect_err("a refusal");

    let message = error.to_string();
    assert!(
        message.contains("/no/such/checkout"),
        "unexpected: {message}"
    );
    assert!(message.contains("does not exist"), "unexpected: {message}");
    assert!(
        ops::list_runs(&store, "report", StepDetail::Counts).expect("the history is readable")
            ["runs"]
            .as_array()
            .is_none_or(|runs| runs.is_empty()),
        "a refused run should leave no record behind"
    );
}
