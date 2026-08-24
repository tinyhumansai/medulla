//! Run-projection and waiting cases for the workflow MCP server.
//!
//! Split out of `cases.rs`: these exercise `workflow_run_get`/`workflow_runs`
//! reading back a settled run rather than the tool surface's shape, and share
//! that file's fixtures.

use super::cases::*;

/// Record a settled run of `sweep` whose one step emitted a large output.
///
/// Written straight to the store rather than run: starting a real run needs a
/// coding-agent CLI on `PATH`, and what these tests are about is the *shape* of
/// what comes back, not the execution that produced it.
fn record_a_run(store: &Arc<dyn WorkflowStore>, run_id: &str) {
    let mut record = crate::workflows::new_run_record(run_id, "sweep", 1_000);
    record.status = crate::workflows::RunStatus::Succeeded;
    record.finished_at = Some(2_000);
    record.steps = vec![crate::workflows::RunStep {
        node_id: "work".to_string(),
        status: "ok".to_string(),
        duration_ms: 900,
        input: Some(json!("x".repeat(100_000))),
        output: Some(json!("y".repeat(100_000))),
        diagnostics: Vec::new(),
        transcript: Vec::new(),
    }];
    store.record_run(&record).expect("records the run");
}

#[tokio::test]
async fn reading_one_run_summarizes_it_unless_the_whole_thing_is_asked_for() {
    let (_root, store) = store();
    call(
        &store,
        "workflow_create",
        json!({ "id": "sweep", "document": document("sweep") }),
    )
    .await;
    record_a_run(&store, "run-1");

    let (summary, is_error) = call(&store, "workflow_run_get", json!({ "runId": "run-1" })).await;
    assert!(!is_error, "{summary}");
    // The default answers "what happened" — the step is there, its output is
    // bounded, and the 100KB prompt that says nothing about the outcome is not.
    assert_eq!(summary["stepDetail"], "summary");
    assert_eq!(summary["steps"][0]["nodeId"], "work");
    assert!(tinyflows::store::is_truncated(
        &summary["steps"][0]["output"]
    ));
    assert!(summary["steps"][0].get("input").is_none(), "{summary}");

    let (full, is_error) = call(
        &store,
        "workflow_run_get",
        json!({ "runId": "run-1", "steps": "full" }),
    )
    .await;
    assert!(!is_error, "{full}");
    assert_eq!(full["stepDetail"], "full");
    assert_eq!(full["steps"][0]["input"].as_str().unwrap().len(), 100_000);
}

#[tokio::test]
async fn listing_a_workflows_runs_does_not_inline_a_single_step() {
    let (_root, store) = store();
    call(
        &store,
        "workflow_create",
        json!({ "id": "sweep", "document": document("sweep") }),
    )
    .await;
    record_a_run(&store, "run-1");
    record_a_run(&store, "run-2");

    let (runs, is_error) = call(&store, "workflow_runs", json!({ "id": "sweep" })).await;

    assert!(!is_error, "{runs}");
    let listed = runs["runs"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert!(
        listed.iter().all(|run| run.get("steps").is_none()),
        "{runs}"
    );
    // The complaint this answers: a history listing of a real workflow came
    // back as 433KB, which is more than the model reading it can hold.
    let bytes = serde_json::to_string(&runs).unwrap().len();
    assert!(
        bytes < 2_000,
        "a two-run listing should not cost {bytes} bytes"
    );
}

#[tokio::test]
async fn an_unreadable_step_level_is_refused_rather_than_guessed_at() {
    let (_root, store) = store();

    // A mis-shaped *argument* is an invalid-params error, the same as a missing
    // `id` or a non-object `inputs`: the caller wrote the call wrong, which is
    // a different thing from the tool having run and failed.
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "workflow_run_get",
            "arguments": { "runId": "run-1", "steps": "brief" },
        },
    });
    let response = handle_request(&session(&store, ToolMode::Full), &request)
        .await
        .expect("a response");

    assert_eq!(response["error"]["code"], -32602);
    // Named, so the next call is right rather than another guess.
    for level in ["full", "summary", "counts"] {
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains(level),
            "{response}"
        );
    }
}

#[tokio::test]
async fn a_wait_budget_that_is_not_a_duration_is_refused() {
    let (_root, store) = store();
    call(
        &store,
        "workflow_create",
        json!({ "id": "sweep", "document": document("sweep") }),
    )
    .await;

    // Rejected before anything runs, and without falling back to a blocking
    // wait: guessing what a caller meant by `-1` is guessing how long to hold
    // its call open for.
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "workflow_run",
            "arguments": { "id": "sweep", "waitMs": -1 },
        },
    });
    let response = handle_request(&session(&store, ToolMode::Full), &request)
        .await
        .expect("a response");

    assert_eq!(response["error"]["code"], -32602);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("waitMs"),
        "{response}"
    );
    // Nothing was admitted, so nothing is in the history to poll.
    let (runs, _) = call(&store, "workflow_runs", json!({ "id": "sweep" })).await;
    assert!(runs["runs"].as_array().unwrap().is_empty(), "{runs}");
}
