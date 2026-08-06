//! The capability model: what a grant permits, and what nothing outside it can
//! change.
//!
//! These are the tests that pin the exclusivity claim. If the depth carried on a
//! grant could be raised by anything a caller sends, or one holder could reach
//! another's tasks, the whole guard is decorative.

use std::sync::Arc;

use serde_json::json;

use super::super::grants::{Grant, GrantRegistry};
use super::super::runs::{HarnessRunRegistry, HarnessRunStatus};
use super::super::server::{handle_control, SessionState, TaskRegistry};
use super::super::types::{FleetOps, ToolFamilies, PROTOCOL_VERSION};
use super::FakeFleet;

/// Drive one op against a fresh connection holding `token`.
async fn call(
    ops: &Arc<dyn FleetOps>,
    grants: &GrantRegistry,
    registry: &TaskRegistry,
    token: &str,
    op: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let runs = HarnessRunRegistry::new();
    call_with_runs(ops, grants, registry, &runs, token, op, params).await
}

/// As [`call`], against a caller-owned run registry.
///
/// Separate because retirement spans several calls: the run a session leaves
/// executing has to still be there when the report that settles it arrives.
#[allow(clippy::too_many_arguments)]
async fn call_with_runs(
    ops: &Arc<dyn FleetOps>,
    grants: &GrantRegistry,
    registry: &TaskRegistry,
    runs: &HarnessRunRegistry,
    token: &str,
    op: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let mut session = SessionState::default();
    let hello = json!({
        "v": PROTOCOL_VERSION, "id": 1, "op": "hello",
        "params": { "protocol": PROTOCOL_VERSION, "token": token },
    });
    handle_control(ops, grants, registry, runs, &mut session, &hello).await;
    let request = json!({ "v": PROTOCOL_VERSION, "id": 2, "op": op, "params": params });
    handle_control(ops, grants, registry, runs, &mut session, &request).await
}

#[test]
fn a_minted_token_redeems_to_exactly_what_was_minted() {
    let grants = GrantRegistry::new();
    let grant = Grant::new("session-a", 1, 3)
        .with_families(ToolFamilies::workflows_only())
        .with_max_in_flight(7);

    let token = grants.mint(grant.clone());

    assert_eq!(grants.redeem(&token), Some(grant));
}

#[test]
fn two_grants_never_share_a_token() {
    let grants = GrantRegistry::new();

    let first = grants.mint(Grant::new("a", 0, 2));
    let second = grants.mint(Grant::new("b", 0, 2));

    assert_ne!(first, second);
    assert_eq!(grants.redeem(&first).unwrap().session, "a");
    assert_eq!(grants.redeem(&second).unwrap().session, "b");
}

#[test]
fn a_token_is_long_enough_not_to_be_guessed() {
    let grants = GrantRegistry::new();

    // 32 bytes rendered as hex. Short enough to pass through an environment
    // variable, long enough that guessing is not a strategy.
    assert_eq!(grants.mint(Grant::new("a", 0, 2)).len(), 64);
}

#[test]
fn revoking_a_session_drops_only_its_grants() {
    let grants = GrantRegistry::new();
    let doomed = grants.mint(Grant::new("ending", 0, 2));
    let survivor = grants.mint(Grant::new("continuing", 0, 2));

    grants.revoke("ending");

    assert_eq!(grants.redeem(&doomed), None);
    assert!(grants.redeem(&survivor).is_some());
}

#[test]
fn depth_decides_whether_a_grant_may_dispatch_at_all() {
    assert!(Grant::new("s", 0, 2).may_dispatch());
    assert!(Grant::new("s", 1, 2).may_dispatch());
    // At the ceiling the verb is withheld rather than the call refused politely.
    assert!(!Grant::new("s", 2, 2).may_dispatch());
    assert!(!Grant::new("s", 3, 2).may_dispatch());
    // A ceiling of zero means no spawned harness may dispatch at all.
    assert!(!Grant::new("s", 0, 0).may_dispatch());
}

#[test]
fn a_child_sits_one_level_deeper_and_the_count_cannot_wrap() {
    assert_eq!(Grant::new("s", 0, 2).child_depth(), 1);
    assert_eq!(Grant::new("s", u8::MAX, 2).child_depth(), u8::MAX);
}

#[tokio::test]
async fn nothing_in_a_request_can_raise_a_grants_depth() {
    // The test the exclusivity claim rests on. Depth lives server-side, keyed by
    // token; a caller that sends its own is sending something nothing reads.
    let grants = GrantRegistry::new();
    let token = grants.mint(Grant::new("deep", 2, 2));
    let ops: Arc<dyn FleetOps> = Arc::new(FakeFleet::new());
    let registry = TaskRegistry::new();

    for forged in [
        json!({ "instruction": "x", "depth": 0 }),
        json!({ "instruction": "x", "maxDepth": 99 }),
        json!({ "instruction": "x", "grant": { "depth": 0, "maxDepth": 99 } }),
    ] {
        let response = call(&ops, &grants, &registry, &token, "task.dispatch", forged).await;
        assert_eq!(response["error"]["kind"], json!("depthExceeded"));
    }
}

#[tokio::test]
async fn one_grant_cannot_read_or_abort_anothers_tasks() {
    let grants = GrantRegistry::new();
    let mine = grants.mint(Grant::new("mine", 0, 2));
    let theirs = grants.mint(Grant::new("theirs", 0, 2));
    let ops: Arc<dyn FleetOps> = Arc::new(FakeFleet::new());
    let registry = TaskRegistry::new();

    let dispatched = call(
        &ops,
        &grants,
        &registry,
        &mine,
        "task.dispatch",
        json!({ "instruction": "x" }),
    )
    .await;
    let task_id = dispatched["result"]["taskId"].as_str().unwrap().to_string();

    for op in ["task.get", "task.abort"] {
        let response = call(
            &ops,
            &grants,
            &registry,
            &theirs,
            op,
            json!({ "taskId": task_id.clone() }),
        )
        .await;
        assert_eq!(response["error"]["kind"], json!("noSuchTask"), "{op}");
    }

    // And the other holder's listing does not mention it either.
    let listed = call(&ops, &grants, &registry, &theirs, "task.list", json!({})).await;
    assert_eq!(listed["result"]["tasks"], json!([]));
}

#[tokio::test]
async fn a_grant_at_its_concurrency_ceiling_sheds_rather_than_queues() {
    let grants = GrantRegistry::new();
    let token = grants.mint(Grant::new("busy", 0, 2).with_max_in_flight(1));
    let ops: Arc<dyn FleetOps> = Arc::new(FakeFleet::new().with_outcome(super::FakeOutcome::Hang));
    let registry = TaskRegistry::new();

    let first = call(
        &ops,
        &grants,
        &registry,
        &token,
        "task.dispatch",
        json!({ "instruction": "first" }),
    )
    .await;
    assert_eq!(first["ok"], json!(true));

    let second = call(
        &ops,
        &grants,
        &registry,
        &token,
        "task.dispatch",
        json!({ "instruction": "second" }),
    )
    .await;

    assert_eq!(second["error"]["kind"], json!("tooManyInFlight"));
    assert_eq!(second["error"]["retryable"], json!(true));
}

#[tokio::test]
async fn concurrent_connections_reserve_one_in_flight_slot_atomically() {
    let grants = GrantRegistry::new();
    let token = grants.mint(Grant::new("busy", 0, 2).with_max_in_flight(1));
    let ops: Arc<dyn FleetOps> = Arc::new(FakeFleet::new().with_outcome(super::FakeOutcome::Hang));
    let registry = TaskRegistry::new();

    let (first, second) = tokio::join!(
        call(
            &ops,
            &grants,
            &registry,
            &token,
            "task.dispatch",
            json!({ "instruction": "first" }),
        ),
        call(
            &ops,
            &grants,
            &registry,
            &token,
            "task.dispatch",
            json!({ "instruction": "second" }),
        ),
    );

    let successes = [&first, &second]
        .into_iter()
        .filter(|response| response["ok"] == json!(true))
        .count();
    let shed = [&first, &second]
        .into_iter()
        .filter(|response| response["error"]["kind"] == json!("tooManyInFlight"))
        .count();
    assert_eq!((successes, shed), (1, 1));
    assert_eq!(registry.in_flight(&token), 1);
}

#[test]
fn a_concurrency_ceiling_is_never_zero() {
    // Zero would advertise the tool and refuse every call, which reads as broken
    // rather than disabled. Turning the family off is what `fleetTools` is for.
    assert_eq!(Grant::new("s", 0, 2).with_max_in_flight(0).max_in_flight, 1);
}

#[test]
fn tool_families_gate_by_prefix() {
    let both = ToolFamilies::default();
    assert!(both.allows("workflow_list"));
    assert!(both.allows("fleet_dispatch"));

    let workflows = ToolFamilies::workflows_only();
    assert!(workflows.allows("workflow_list"));
    assert!(!workflows.allows("fleet_dispatch"));

    // A name in neither family is not silently withheld by a check that predates
    // it — the families gate the two surfaces that exist, nothing more.
    assert!(workflows.allows("something_else"));
}

#[tokio::test]
async fn a_reporting_only_grant_may_report_its_run_and_nothing_else() {
    let ops: Arc<dyn FleetOps> = Arc::new(FakeFleet::new());
    let grants = GrantRegistry::new();
    let registry = TaskRegistry::new();
    let runs = HarnessRunRegistry::new();
    let token = grants.mint(Grant::new("session-a", 0, 3));

    let started = json!({ "runId": "run-1", "workflowId": "review", "status": "running" });
    let response = call_with_runs(
        &ops,
        &grants,
        &registry,
        &runs,
        &token,
        "run.report",
        started,
    )
    .await;
    assert_eq!(response["ok"], json!(true));

    // The harness exits with the run still going. This is the state
    // `mcp::revoke_session` leaves behind rather than a full revoke.
    assert!(runs.retire("session-a"));
    grants.restrict_to_reporting("session-a");

    // Reporting survives — the whole reason the grant was kept.
    let progress = json!({
        "runId": "run-1", "workflowId": "review",
        "status": "running", "detail": "step 2",
    });
    let response = call_with_runs(
        &ops,
        &grants,
        &registry,
        &runs,
        &token,
        "run.report",
        progress,
    )
    .await;
    assert_eq!(response["ok"], json!(true));
    assert_eq!(
        runs.for_session("session-a")[0].detail.as_deref(),
        Some("step 2")
    );

    // Nothing else does. A subprocess whose session is over must not be able to
    // start work, poll work, or mint a child capability out of what it kept.
    for op in ["task.dispatch", "task.list", "worker.list", "grant.child"] {
        let response = call_with_runs(
            &ops,
            &grants,
            &registry,
            &runs,
            &token,
            op,
            json!({ "instruction": "anything" }),
        )
        .await;
        assert_eq!(response["ok"], json!(false), "{op} was served");
        assert_eq!(
            response["error"]["kind"],
            json!("unauthenticated"),
            "{op} was refused for the wrong reason"
        );
    }
}

#[tokio::test]
async fn settling_the_last_run_gives_the_reporting_grant_back() {
    let ops: Arc<dyn FleetOps> = Arc::new(FakeFleet::new());
    let grants = GrantRegistry::new();
    let registry = TaskRegistry::new();
    let runs = HarnessRunRegistry::new();
    let token = grants.mint(Grant::new("session-a", 0, 3));

    call_with_runs(
        &ops,
        &grants,
        &registry,
        &runs,
        &token,
        "run.report",
        json!({ "runId": "run-1", "workflowId": "review", "status": "running" }),
    )
    .await;
    assert!(runs.retire("session-a"));
    grants.restrict_to_reporting("session-a");

    let response = call_with_runs(
        &ops,
        &grants,
        &registry,
        &runs,
        &token,
        "run.report",
        json!({ "runId": "run-1", "workflowId": "review", "status": "succeeded" }),
    )
    .await;
    assert_eq!(response["ok"], json!(true));
    // The outcome is recorded, and the token that recorded it is gone: there is
    // nothing further this session can ever say.
    assert_eq!(
        runs.for_session("session-a")[0].status,
        HarnessRunStatus::Succeeded
    );
    assert!(grants.redeem(&token).is_none());
    let response = call_with_runs(
        &ops,
        &grants,
        &registry,
        &runs,
        &token,
        "run.report",
        json!({ "runId": "run-1", "workflowId": "review", "status": "running" }),
    )
    .await;
    assert_eq!(response["ok"], json!(false));
}
