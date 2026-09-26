//! Tests for the workflow-run tool's availability and refusal behavior.

use super::cases::*;

#[tokio::test]
async fn a_run_can_be_started_followed_and_stopped_from_the_same_surface() {
    // The three verbs a caller that starts a run over MCP needs to see it
    // through. Cancelling used to be deliberately absent, on the reasoning that
    // whoever started a run was watching it — which stopped being true once a
    // model could start one and answer with the id.
    for verb in ["workflow_run", "workflow_run_detail", "workflow_run_cancel"] {
        assert!(TOOL_NAMES.contains(&verb), "{verb} is missing");
    }
    // Resuming a parked run is still the operator's own action: an approval
    // gate exists to put a person in the loop, and a verb that steps past it
    // would take them back out.
    assert!(!TOOL_NAMES.contains(&"workflow_resume"));
}

#[tokio::test]
async fn cancelling_a_run_that_does_not_exist_explains_itself_rather_than_failing() {
    let (_root, store) = store();

    let (result, is_error) = call(
        &store,
        "workflow_run_cancel",
        json!({ "runId": "run-nobody-is-running" }),
    )
    .await;

    // Not an error: a run that already settled, or one whose id was mistyped,
    // is a normal thing to aim a cancel at, and reporting it as a broken call
    // would invite a retry that can never succeed.
    assert!(!is_error, "{result}");
    assert_eq!(result["cancelled"], false);
    assert_eq!(result["runId"], "run-nobody-is-running");
    let reason = result["reason"].as_str().unwrap_or_default();
    assert!(reason.contains("no run with this id exists"), "{reason}");
}

#[tokio::test]
async fn cancelling_without_a_run_id_is_a_protocol_error_naming_the_argument() {
    let (_root, store) = store();
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "workflow_run_cancel", "arguments": {} },
    });

    let response = handle_request(&session(&store, ToolMode::Full), &request)
        .await
        .expect("a response");

    assert_eq!(response["error"]["code"], -32602);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("runId"),
        "{response}"
    );
}

#[tokio::test]
async fn run_detail_answers_with_the_record_and_the_live_half_beside_it() {
    let (_root, store) = store();
    let mut record = crate::workflows::new_run_record("run-1", "sweep", 1_000);
    record.status = crate::workflows::RunStatus::Running;
    store.record_run(&record).expect("records the run");

    let (result, is_error) = call(&store, "workflow_run_detail", json!({ "runId": "run-1" })).await;

    assert!(!is_error, "{result}");
    assert_eq!(result["run"]["id"], "run-1");
    // The join key, so a reader can grep a worker's own logs for this run.
    assert_eq!(result["live"]["taskIdPrefix"], "wf:run-1:");
    // These sessions have no fleet behind them, and the answer says so rather
    // than reporting an empty list as though nothing were running.
    assert!(result["live"]["fleetUnavailable"].is_string(), "{result}");
}

#[tokio::test]
async fn an_unreadable_step_level_is_refused_by_run_detail_too() {
    let (_root, store) = store();
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "workflow_run_detail",
            "arguments": { "runId": "run-1", "steps": "brief" },
        },
    });

    let response = handle_request(&session(&store, ToolMode::Full), &request)
        .await
        .expect("a response");

    assert_eq!(response["error"]["code"], -32602);
}

#[tokio::test]
async fn running_a_workflow_that_is_disabled_is_refused_rather_than_silently_skipped() {
    let (_root, store) = store();
    let disabled = json!({
        "id": "paused", "name": "Paused", "enabled": false,
        "nodes": [{ "id": "t", "kind": "trigger", "name": "start",
                    "config": { "trigger_kind": "manual" } }],
        "edges": []
    })
    .to_string();
    call(
        &store,
        "workflow_create",
        json!({ "id": "paused", "document": disabled }),
    )
    .await;

    let (result, is_error) = call(&store, "workflow_run", json!({ "id": "paused" })).await;
    assert!(is_error, "{result}");
    assert!(
        result["error"]
            .as_str()
            .unwrap_or_default()
            .contains("disabled"),
        "{result}"
    );
}
