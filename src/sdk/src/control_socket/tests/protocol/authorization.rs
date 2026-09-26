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

/// The other door into the recursion a workflow `agent` node's withheld
/// workflow surface exists to close.
///
/// A node's harness keeps `fleet_*` on purpose — the fleet verbs are bounded by
/// the grant's own depth and in-flight limits, and taking them from a node that
/// legitimately delegates would cost something for nothing — but
/// `fleet_dispatch` carries a `workflow` argument. Without this refusal the node
/// could read the id and fingerprint out of `fleet_workers` and dispatch the
/// very graph it is a step of through the fleet, which is the run it was
/// launched without `workflow_run` precisely to prevent.
#[tokio::test]
async fn a_fleet_only_grant_cannot_dispatch_a_saved_workflow() {
    let mut harness = Harness::with(
        FakeFleet::new(),
        Grant::new("s", 0, 2).with_families(ToolFamilies::fleet_only()),
    );

    let response = harness
        .call(
            "task.dispatch",
            json!({
                "instruction": "babysit the PR",
                "workflow": "pr-babysitter",
                "workflowFingerprint": "babysitter-fingerprint",
            }),
        )
        .await;

    assert_eq!(kind(&response), "unauthenticated");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("workflow tools"),
        "the refusal must say which authority is missing: {response}"
    );
    assert!(
        harness.fake.dispatched.lock().unwrap().is_empty(),
        "nothing may reach the fleet"
    );
}

/// And the fleet surface it was deliberately left is untouched: the refusal is
/// scoped to a workflow-bearing dispatch, not to dispatching at all.
#[tokio::test]
async fn a_fleet_only_grant_may_still_delegate_an_ordinary_instruction() {
    let mut harness = Harness::with(
        FakeFleet::new(),
        Grant::new("s", 0, 2).with_families(ToolFamilies::fleet_only()),
    );

    let response = harness
        .call("task.dispatch", json!({ "instruction": "run the tests" }))
        .await;

    assert_eq!(response["result"]["status"], json!("running"), "{response}");
}
