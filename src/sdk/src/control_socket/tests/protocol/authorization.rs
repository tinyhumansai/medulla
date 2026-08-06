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

#[tokio::test]
async fn a_hook_only_grant_can_file_a_report_but_nothing_else() {
    // This is the credential a launched harness's hook commands actually get
    // (see `medulla::mcp::attach::local_hook_grant`), written straight into the
    // harness's own environment rather than kept in a file only the MCP
    // subprocess can read — so every op but the one that carries no authority
    // at all must stay refused no matter what the caller asks for.
    let mut harness = Harness::with(FakeFleet::new(), Grant::hook_only("s"));

    let report = harness
        .call(
            "hook.report",
            json!({ "event": "Stop", "summary": "turn finished" }),
        )
        .await;
    assert_eq!(report["ok"], json!(true), "{report}");

    for (op, params) in [
        ("worker.list", json!({})),
        ("task.list", json!({})),
        ("task.dispatch", json!({ "instruction": "x" })),
        ("grant.child", json!({})),
    ] {
        let response = harness.call(op, params).await;
        assert_eq!(kind(&response), "unauthenticated", "{op} was answered");
    }
}
