//! Tests for the workflow-run tool's availability and refusal behavior.

use super::cases::*;

#[tokio::test]
async fn running_a_workflow_is_on_the_belt_but_cancelling_one_is_not() {
    assert!(TOOL_NAMES.contains(&"workflow_run"));
    for forbidden in ["workflow_cancel", "workflow_cancel_run", "workflow_resume"] {
        assert!(
            !TOOL_NAMES.contains(&forbidden),
            "{forbidden} must not be on the authoring belt"
        );
    }
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
