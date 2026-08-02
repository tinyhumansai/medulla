//! Workflow dispatch checks that bind a requested id to the selected worker's
//! exact advertised definition.

use std::time::Duration;

use serde_json::json;

use super::super::grants::Grant;
use super::protocol::{kind, Harness};
use super::FakeFleet;

#[tokio::test]
async fn the_roster_exposes_each_workers_workflow_definition() {
    let response = Harness::new().call("worker.list", json!({})).await;
    let workflow = &response["result"]["workers"][0]["workflows"][0];

    assert_eq!(workflow["id"], json!("release"));
    assert_eq!(workflow["fingerprint"], json!("release-fingerprint"));
}

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

#[tokio::test]
async fn an_unresponsive_workers_workflow_probe_does_not_hide_the_roster() {
    let fleet = FakeFleet::new()
        .with_workers(vec![
            crate::control_socket::FleetWorker {
                id: "alpha".into(),
                address: "alpha-address".into(),
                ..Default::default()
            },
            crate::control_socket::FleetWorker {
                id: "beta".into(),
                address: "beta-address".into(),
                ..Default::default()
            },
        ])
        .with_unresponsive_workflow_worker("alpha-address");
    let mut harness = Harness::with(fleet, Grant::new("s", 0, 2));

    let response = tokio::time::timeout(
        Duration::from_secs(3),
        harness.call("worker.list", json!({})),
    )
    .await
    .expect("the roster must answer inside the control-client deadline");

    assert_eq!(response["result"]["workers"][0]["id"], json!("alpha"));
    assert!(response["result"]["workers"][0].get("workflows").is_none());
    assert_eq!(
        response["result"]["workers"][1]["workflows"][0]["id"],
        json!("release")
    );
}
