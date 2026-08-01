//! Tests for the live-hub [`FleetOps`], chiefly that a task dispatched through
//! the fleet tools is *visible* — attributed to a lane and reporting progress
//! the way an operator-started task does.

use super::hub_ops::{activity_key, record_outcome, tee_status};
use super::{FleetDefaults, HubFleetOps, HubSlot};
use crate::control_socket::FleetOps;
use crate::hub::{ActivityLog, RunError, TaskOutcome, TaskRequest};
use crate::tinyplace::TokenUsage;

/// A request naming `worker`, with the ids the control plane would have minted.
fn request(worker: &str) -> TaskRequest {
    TaskRequest {
        task_id: "mcp-wire-1".into(),
        abort_id: "mcp-abort-1".into(),
        cycle_id: Some("mcp:session".into()),
        instruction: "do the thing".into(),
        worker_address: worker.into(),
        provider: None,
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        conversation: None,
        fleet_depth: 1,
    }
}

/// A settled dispatch carrying `reply`.
fn done(reply: &str) -> Result<TaskOutcome, RunError> {
    Ok(TaskOutcome {
        reply: reply.into(),
        usage: TokenUsage {
            input_tokens: 1,
            output_tokens: 2,
        },
        harness: None,
    })
}

#[tokio::test]
async fn a_dispatch_with_no_hub_says_so_rather_than_hanging() {
    // The slot is empty until the hub connects. A caller must get an answer it
    // can act on rather than a dispatch parked on a handle that may never come.
    let ops = HubFleetOps::new(HubSlot::default(), FleetDefaults::default());

    let outcome = ops.dispatch(request("alpha"), None).await;

    let error = outcome.expect_err("no hub means no dispatch");
    assert!(error.to_string().contains("not connected"), "{error}");
}

#[test]
fn an_unconnected_hub_is_not_an_empty_fleet() {
    // `None` and `[]` mean different things to a caller: one says ask again, the
    // other says give up. Collapsing them is the bug this guards.
    let ops = HubFleetOps::new(HubSlot::default(), FleetDefaults::default());

    assert!(ops.workers().is_none());
}

#[test]
fn mcp_activity_uses_the_cycle_and_abort_handle_the_ui_can_cancel() {
    assert_eq!(activity_key(&request("alpha")), "mcp:session/t:mcp-abort-1");
}

#[test]
fn the_default_worker_falls_back_to_this_host() {
    let ops = HubFleetOps::new(
        HubSlot::default(),
        FleetDefaults {
            worker_address: Some("this-device".into()),
        },
    );

    assert_eq!(ops.default_worker().as_deref(), Some("this-device"));
}

#[tokio::test]
async fn status_frames_reach_the_poller_and_the_activity_log_both() {
    // The tee is the whole point: `fleet_result` reads the poller's copy for its
    // progress, and the Agents view reads the log's. Forwarding to one only
    // would leave the other blind.
    let activity = ActivityLog::new();
    activity.dispatched("mcp-wire-1", "alpha");
    let (poller, mut polled) = tokio::sync::mpsc::unbounded_channel::<String>();

    let tee = tee_status(activity.clone(), "mcp-wire-1".into(), Some(poller));
    tee.send("reading files".to_string()).unwrap();
    tee.send("writing a patch".to_string()).unwrap();
    drop(tee);

    assert_eq!(polled.recv().await.as_deref(), Some("reading files"));
    assert_eq!(polled.recv().await.as_deref(), Some("writing a patch"));

    let seen: Vec<String> = activity
        .snapshot()
        .into_iter()
        .filter(|entry| entry.task_id == "mcp-wire-1" && entry.kind == "status")
        .map(|entry| entry.content)
        .collect();
    assert_eq!(seen, ["reading files", "writing a patch"]);
}

#[tokio::test]
async fn a_task_the_operator_never_started_still_lands_on_a_lane() {
    // Without the attribution, work dispatched by a harness runs on somebody's
    // machine and appears against no worker at all — invisible in the Agents
    // view, so nobody can see it or stop it.
    let activity = ActivityLog::new();
    activity.dispatched("mcp-wire-1", "alpha");

    let tee = tee_status(activity.clone(), "mcp-wire-1".into(), None);
    tee.send("on it".to_string()).unwrap();
    drop(tee);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let entry = activity
        .snapshot()
        .into_iter()
        .find(|entry| entry.task_id == "mcp-wire-1")
        .expect("the task should appear in the activity log");
    assert_eq!(entry.agent_id, "alpha", "it must belong to a worker's lane");
}

#[test]
fn a_settled_dispatch_is_recorded_as_a_reply() {
    let activity = ActivityLog::new();
    activity.dispatched("mcp-wire-1", "alpha");

    record_outcome(&activity, "mcp-wire-1", &done("the work is done"));

    let entry = activity
        .snapshot()
        .into_iter()
        .find(|entry| entry.kind == "reply")
        .expect("a terminal reply");
    assert_eq!(entry.content, "the work is done");
    assert_eq!(entry.agent_id, "alpha");
}

#[test]
fn a_failed_dispatch_is_recorded_as_an_error_the_operator_can_read() {
    let activity = ActivityLog::new();
    activity.dispatched("mcp-wire-1", "alpha");

    record_outcome(
        &activity,
        "mcp-wire-1",
        &Err(RunError::Held("operator is working there".into())),
    );

    let entry = activity
        .snapshot()
        .into_iter()
        .find(|entry| entry.kind == "error")
        .expect("a terminal error");
    assert!(
        entry.content.contains("operator is working there"),
        "{entry:?}"
    );
}
