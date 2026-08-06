//! Tests for the run-event to progress-notification mapping.
//!
//! The point of a progress notification is that it arrives at all — a client's
//! idle timer resets on any of them — so these check the two things that would
//! stop that happening: an event kind silently not forwarded, and a payload
//! shape the mapping does not recognise.

use serde_json::json;

use super::*;
use crate::harness_work::kinds;

/// A plan payload, as `WorkflowRunObserver` emits one.
fn plan(states: &[(&str, &str)]) -> Value {
    let steps: Vec<Value> = states
        .iter()
        .map(|(label, status)| json!({ "content": label, "status": status }))
        .collect();
    json!({ "goal": "workflow sweep (run run-1)", "steps": steps })
}

/// The same list under the key `todo_update` uses.
fn todos(states: &[(&str, &str)]) -> Value {
    json!({ "todos": plan(states)["steps"], "source": "workflow" })
}

#[test]
fn the_starting_plan_reports_how_much_there_is_to_do() {
    let (message, total) = line(
        "sweep",
        kinds::PLAN_UPDATE,
        &plan(&[("Fetch", "pending"), ("Work", "pending")]),
    )
    .expect("a plan is worth a notification");

    assert_eq!(total, Some(2));
    assert!(message.contains("sweep"), "{message}");
    assert!(message.contains('2'), "{message}");
}

#[test]
fn a_settled_step_reports_the_count_and_what_is_running_now() {
    let (message, total) = line(
        "sweep",
        kinds::TODO_UPDATE,
        &todos(&[
            ("Fetch", "completed"),
            ("Work", "in_progress"),
            ("Report", "pending"),
        ]),
    )
    .expect("a settled step is worth a notification");

    assert_eq!(total, Some(3));
    assert!(message.contains("1/3"), "{message}");
    // The node's own name, so a caller watching a long run can tell which step
    // is the slow one rather than only that something is.
    assert!(message.contains("Work"), "{message}");
}

#[test]
fn a_finished_run_reports_the_summary_the_observer_wrote() {
    let (message, total) = line(
        "sweep",
        kinds::RUN_RESULT,
        &json!({ "ok": true, "summary": "ran 3 steps in 2s" }),
    )
    .expect("a finished run is worth a notification");

    assert_eq!(total, None);
    assert!(message.contains("ran 3 steps in 2s"), "{message}");
}

#[test]
fn a_run_with_no_step_running_yet_still_reports_progress() {
    // Between the last step settling and the next starting there is nothing
    // `in_progress`. Dropping the notification there would drop the heartbeat
    // at exactly the moment a long run is between two slow steps.
    let (message, _) = line(
        "sweep",
        kinds::TODO_UPDATE,
        &todos(&[("Fetch", "completed"), ("Work", "completed")]),
    )
    .expect("a notification");

    assert!(message.contains("2/2"), "{message}");
}

#[test]
fn the_noisy_kinds_are_not_forwarded() {
    // A workflow dispatching ten agents emits these constantly. Relaying them
    // would turn a heartbeat into a firehose in the client's transcript.
    for noisy in [
        kinds::FILE_CHANGE,
        kinds::SUBAGENT_START,
        kinds::SESSION_INFO,
    ] {
        assert!(
            line("sweep", noisy, &json!({ "path": "a.rs" })).is_none(),
            "{noisy} must not be forwarded"
        );
    }
}

#[test]
fn a_payload_without_a_plan_is_reported_rather_than_dropped() {
    // Better a heartbeat that says "0/0" than a client timing out because the
    // observer's payload shape moved.
    let (message, total) = line("sweep", kinds::TODO_UPDATE, &json!({})).expect("a notification");

    assert_eq!(total, Some(0));
    assert!(message.contains("0/0"), "{message}");
}

#[tokio::test]
async fn the_sink_sends_one_progress_notification_per_event() {
    let (outbound, mut inbound) = tokio::sync::mpsc::channel(8);
    let progress = crate::mcp::progress::Progress::from_params(
        &json!({ "_meta": { "progressToken": "t-1" } }),
        Some(&outbound),
    )
    .expect("a progress channel");
    let sink = sink(progress, "sweep");

    sink(kinds::PLAN_UPDATE, plan(&[("Work", "pending")]));
    sink(kinds::FILE_CHANGE, json!({ "path": "a.rs" }));
    sink(kinds::TODO_UPDATE, todos(&[("Work", "completed")]));

    let first = inbound.recv().await.expect("a notification");
    assert_eq!(first["method"], "notifications/progress");
    assert_eq!(first["params"]["progressToken"], "t-1");
    assert_eq!(first["params"]["progress"], 1);
    let second = inbound.recv().await.expect("a second notification");
    // The specification requires this to increase per token, and the file
    // change in between is not one of them.
    assert_eq!(second["params"]["progress"], 2);
    assert!(second["params"]["message"]
        .as_str()
        .unwrap()
        .contains("1/1"));
    assert!(inbound.try_recv().is_err(), "only the two run events");
}

#[test]
fn a_client_that_asked_for_no_progress_gets_no_channel() {
    let (outbound, _inbound) = tokio::sync::mpsc::channel(1);

    // No `_meta` at all, and an explicit null token: both are a client that has
    // no correlation for a notification and would only be confused by one.
    assert!(crate::mcp::progress::Progress::from_params(
        &json!({ "name": "workflow_run" }),
        Some(&outbound)
    )
    .is_none());
    assert!(crate::mcp::progress::Progress::from_params(
        &json!({ "_meta": { "progressToken": null } }),
        Some(&outbound)
    )
    .is_none());
    // And a session with no transport behind it cannot report at all.
    assert!(crate::mcp::progress::Progress::from_params(
        &json!({ "_meta": { "progressToken": "t-1" } }),
        None
    )
    .is_none());
}
