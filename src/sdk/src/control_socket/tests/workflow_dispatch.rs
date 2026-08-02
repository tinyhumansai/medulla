//! Workflow dispatch checks that bind a requested id to the selected worker's
//! exact advertised definition.

use serde_json::json;

use super::protocol::{kind, Harness};

#[tokio::test]
async fn a_workflow_dispatch_requires_the_selected_workers_definition_fingerprint() {
    let mut missing = Harness::new();
    let response = missing
        .call(
            "task.dispatch",
            json!({ "instruction": "release", "workflow": "release" }),
        )
        .await;
    assert_eq!(kind(&response), "badRequest");
    assert!(missing.fake.dispatched.lock().unwrap().is_empty());

    let mut stale = Harness::new();
    let response = stale
        .call(
            "task.dispatch",
            json!({
                "instruction": "release",
                "workflow": "release",
                "workflowFingerprint": "different-definition"
            }),
        )
        .await;
    assert_eq!(kind(&response), "badRequest");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("different definition fingerprint"));
    assert!(stale.fake.dispatched.lock().unwrap().is_empty());
}
