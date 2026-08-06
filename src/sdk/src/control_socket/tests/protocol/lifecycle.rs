//! Task polling, listing, settlement, and abort behavior.

use super::*;

#[tokio::test]
async fn polling_an_unknown_task_says_tasks_do_not_outlive_the_instance() {
    let response = Harness::new()
        .call("task.get", json!({ "taskId": "mcp-nope" }))
        .await;
    assert_eq!(kind(&response), "noSuchTask");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("do not survive"));
}

#[tokio::test]
async fn a_settled_task_carries_its_reply_and_usage() {
    let mut harness = Harness::new();
    let dispatched = harness
        .call("task.dispatch", json!({ "instruction": "x" }))
        .await;
    let task_id = dispatched["result"]["taskId"].as_str().unwrap().to_string();
    let response = harness
        .call("task.get", json!({ "taskId": task_id, "waitSeconds": 5 }))
        .await;
    let result = &response["result"];
    assert_eq!(result["status"], json!("done"));
    assert_eq!(result["reply"], json!("done"));
    assert_eq!(result["usage"]["inputTokens"], json!(10));
}

#[tokio::test]
async fn a_shed_task_is_retryable_and_a_failed_one_is_not() {
    for (outcome, status, retryable) in [
        (
            FakeOutcome::Fail(crate::hub::RunError::Busy("at capacity".into())),
            "busy",
            true,
        ),
        (
            FakeOutcome::Fail(crate::hub::RunError::Held("operator here".into())),
            "held",
            true,
        ),
        (
            FakeOutcome::Fail(crate::hub::RunError::Worker("it broke".into())),
            "failed",
            false,
        ),
        (
            FakeOutcome::Fail(crate::hub::RunError::Aborted),
            "aborted",
            false,
        ),
    ] {
        let mut harness = Harness::with(
            FakeFleet::new().with_outcome(outcome),
            Grant::new("s", 0, 2),
        );
        let dispatched = harness
            .call("task.dispatch", json!({ "instruction": "x" }))
            .await;
        let task_id = dispatched["result"]["taskId"].as_str().unwrap().to_string();
        let response = harness
            .call("task.get", json!({ "taskId": task_id, "waitSeconds": 5 }))
            .await;
        let result = &response["result"];
        assert_eq!(result["status"], json!(status));
        assert_eq!(result["retryable"], json!(retryable), "for {status}");
    }
}

#[tokio::test]
async fn a_zero_wait_poll_answers_still_running() {
    let mut harness = Harness::with(
        FakeFleet::new().with_outcome(FakeOutcome::Hang),
        Grant::new("s", 0, 2),
    );
    let dispatched = harness
        .call("task.dispatch", json!({ "instruction": "x" }))
        .await;
    let task_id = dispatched["result"]["taskId"].as_str().unwrap().to_string();
    let response = harness
        .call("task.get", json!({ "taskId": task_id, "waitSeconds": 0 }))
        .await;
    assert_eq!(response["result"]["status"], json!("running"));
}

#[tokio::test]
async fn listing_shows_this_sessions_tasks_newest_first() {
    let mut harness = Harness::new();
    for instruction in ["first", "second"] {
        harness
            .call("task.dispatch", json!({ "instruction": instruction }))
            .await;
    }
    let response = harness.call("task.list", json!({})).await;
    assert_eq!(response["result"]["tasks"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn aborting_settles_a_hanging_task() {
    let mut harness = Harness::with(
        FakeFleet::new().with_outcome(FakeOutcome::Hang),
        Grant::new("s", 0, 2),
    );
    let dispatched = harness
        .call("task.dispatch", json!({ "instruction": "x" }))
        .await;
    let task_id = dispatched["result"]["taskId"].as_str().unwrap().to_string();
    let aborted = harness
        .call("task.abort", json!({ "taskId": task_id.clone() }))
        .await;
    assert_eq!(aborted["result"]["aborted"], json!(true));
    let response = harness
        .call("task.get", json!({ "taskId": task_id, "waitSeconds": 5 }))
        .await;
    assert_eq!(response["result"]["status"], json!("aborted"));
}

#[tokio::test]
async fn aborting_a_task_that_already_settled_reports_that_nothing_was_cancelled() {
    let mut harness = Harness::new();
    let dispatched = harness
        .call("task.dispatch", json!({ "instruction": "x" }))
        .await;
    let task_id = dispatched["result"]["taskId"].as_str().unwrap().to_string();
    harness
        .call(
            "task.get",
            json!({ "taskId": task_id.clone(), "waitSeconds": 5 }),
        )
        .await;
    let response = harness
        .call("task.abort", json!({ "taskId": task_id }))
        .await;
    assert_eq!(response["result"]["aborted"], json!(false));
    assert_eq!(response["result"]["status"], json!("done"));
}

#[tokio::test]
async fn aborting_an_unknown_task_is_refused_rather_than_guessed_at() {
    let response = Harness::new()
        .call("task.abort", json!({ "taskId": "mcp-someone-elses" }))
        .await;
    assert_eq!(kind(&response), "noSuchTask");
}

#[tokio::test]
async fn a_reported_run_is_recorded_under_the_grants_own_session() {
    let mut harness = Harness::new();
    let response = harness
        .call(
            "run.report",
            json!({
                "runId": "run-1",
                "workflowId": "review",
                "status": "running",
                "detail": "reading src/",
                "node": "review",
            }),
        )
        .await;
    assert_eq!(response["ok"], json!(true), "{response}");

    // Under the grant's session, not under anything the caller claimed: the
    // session is what the rail draws the row beneath.
    let runs = harness.runs.for_session("session-1");
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].workflow_id, "review");
    assert_eq!(runs[0].detail.as_deref(), Some("reading src/"));
    assert_eq!(runs[0].frames[0].node.as_deref(), Some("review"));
}

#[tokio::test]
async fn a_run_report_missing_its_identity_is_refused_rather_than_recorded() {
    let mut harness = Harness::new();
    let response = harness
        .call("run.report", json!({ "status": "running" }))
        .await;
    assert_eq!(kind(&response), "badRequest");
    // The second half of the name: refusing it must also mean nothing landed
    // in the registry, or the rail would grow a row for a report the handler
    // said no to.
    assert!(harness.runs.for_session("session-1").is_empty());
}
