//! Dispatch authorization and fleet-depth refusal behavior.

use super::*;

#[tokio::test]
async fn a_grant_without_the_fleet_family_cannot_dispatch() {
    let mut harness = Harness::with(
        FakeFleet::new(),
        Grant::new("s", 0, 2).with_families(ToolFamilies::workflows_only()),
    );
    let response = harness
        .call("task.dispatch", json!({ "instruction": "x" }))
        .await;
    assert_eq!(kind(&response), "unauthenticated");
}

#[tokio::test]
async fn a_grant_at_the_depth_ceiling_is_told_to_do_the_work_itself() {
    let mut harness = Harness::with(FakeFleet::new(), Grant::new("s", 2, 2));
    let response = harness
        .call("task.dispatch", json!({ "instruction": "x" }))
        .await;
    assert_eq!(kind(&response), "depthExceeded");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("do the work"));
}
