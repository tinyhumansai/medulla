//! Deferring a task because a person is working in the checkout.
//!
//! Narrower than it was. A dispatch no longer refuses on meeting a person —
//! their session is simply not a candidate, and a dispatch with nothing else to
//! run in queues behind the checkout's writer. `RunError::Held` is what that
//! queue reports when it outlives the caller's budget, which is the one path
//! left to it and the reason the shape below is unchanged: the backend already
//! reads `harnessHeld` as retryable, and a task that was never attempted must
//! not come back as one that failed.
//!
//! Two halves, and they have to agree. The daemon can only say so in the text of
//! an `error` frame, so [`settle`](super::super::runner) has to recognise that
//! text as occupancy rather than as a task that failed; and the frame the
//! backend receives has to name the reason, or the orchestrator can only guess
//! whether waiting or routing elsewhere is the right move.

use std::time::Duration;

use super::super::socket::{is_retryable, result_frame};
use super::super::types::{RunError, TaskOutcome};
use super::super::TaskRunner;
use super::dispatch::harness::{req, FakeWorker, Mode};

/// The refusal a daemon sends when an operator holds the workspace.
fn held_message() -> String {
    format!(
        "{}: an operator is working in /repos/acme (codex session w_3)",
        crate::daemon::HARNESS_HELD_PREFIX
    )
}

#[tokio::test]
async fn an_operator_hold_settles_as_held_not_as_a_worker_error() {
    // It arrives as an ordinary `error` frame, exactly like backpressure did
    // before it was told apart. Left as `RunError::Worker` it would reach the
    // orchestrator as a task that was attempted and failed — so the work would
    // be abandoned rather than picked up again when the person is done.
    let message = held_message();
    let worker = FakeWorker::new(Mode::Error(message.clone()));
    let runner = TaskRunner::start(worker, Duration::from_millis(5));

    let err = runner.run(req("x"), None).await.expect_err("is refused");
    assert_eq!(err, RunError::Held(message.clone()));
    assert!(
        is_retryable(&err),
        "nothing was attempted, so the task is deferred rather than failed"
    );
    // The daemon's own wording survives, so whoever reads the failure still sees
    // which workspace was occupied and by which session.
    assert_eq!(
        err.to_string(),
        format!("harness held by operator: {message}")
    );
}

#[test]
fn a_held_refusal_names_its_reason_on_the_wire() {
    let err = RunError::Held(held_message());
    let frame = result_frame("t1", &Err(err));

    assert_eq!(frame["taskId"], "t1");
    assert_eq!(frame["ok"], false);
    assert_eq!(frame["retryable"], true);
    // The machine-readable half. `retryable` alone tells the orchestrator to
    // come back; `reason` is what lets it decide to go somewhere else instead of
    // waiting on a person who may have left for the day.
    assert_eq!(frame["reason"], "harnessHeld");
    assert!(frame["retryAfterMs"].as_u64().is_some_and(|ms| ms > 0));
}

#[test]
fn an_ordinary_worker_error_carries_no_reason() {
    // Guards the key leaking onto every failure: `reason` means "this specific
    // thing is in your way", and a `reason` on a plain harness error would be
    // read as one.
    let frame = result_frame("t1", &Err(RunError::Worker("tool exploded".into())));

    assert_eq!(frame["ok"], false);
    assert_eq!(frame["retryable"], false);
    assert!(frame.get("reason").is_none());
    assert!(frame.get("retryAfterMs").is_none());
}

#[test]
fn backpressure_keeps_its_own_shape() {
    // Held and Busy are both "I did not attempt this", but they clear on
    // completely different timescales, so they must not be collapsed into one
    // reason the orchestrator would treat identically.
    let frame = result_frame("t1", &Err(RunError::Busy("daemon at capacity".into())));

    assert_eq!(frame["retryable"], true);
    assert!(frame.get("reason").is_none());
}

#[test]
fn a_successful_task_reports_its_reply_and_usage() {
    let frame = result_frame(
        "t1",
        &Ok(TaskOutcome {
            reply: "done".to_string(),
            usage: crate::protocol::TokenUsage {
                input_tokens: 3,
                output_tokens: 5,
            },
            harness: None,
            session_id: None,
        }),
    );

    assert_eq!(frame["ok"], true);
    assert_eq!(frame["reply"], "done");
    assert_eq!(frame["usage"]["inputTokens"], 3);
    assert_eq!(frame["usage"]["outputTokens"], 5);
    assert!(frame.get("retryable").is_none());
}

/// The half of C2 that costs nothing on the backend: the result names the
/// session that served the task, so a manager's ledger can record *where* the
/// work happened. Additive — the backend's result handler validates nothing
/// beyond `taskId` — and the slot it lands in already exists.
#[test]
fn a_result_reports_the_session_that_served_the_task() {
    let frame = result_frame(
        "t1",
        &Ok(TaskOutcome {
            reply: "done".to_string(),
            usage: crate::protocol::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            harness: None,
            session_id: Some("sess-42".to_string()),
        }),
    );

    assert_eq!(frame["sessionId"], "sess-42");
}

/// A worker that reported no session — one that predates the key, or a workflow
/// run, which is a graph rather than one session — leaves the key absent. Blank
/// counts as absent too: `""` would be recorded as a session id nothing can
/// resume.
#[test]
fn a_result_claims_no_session_the_worker_did_not_report() {
    let outcome = |session_id: Option<&str>| {
        Ok(TaskOutcome {
            reply: "done".to_string(),
            usage: crate::protocol::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            harness: None,
            session_id: session_id.map(str::to_string),
        })
    };

    assert!(result_frame("t1", &outcome(None))
        .get("sessionId")
        .is_none());
    assert!(result_frame("t1", &outcome(Some("   ")))
        .get("sessionId")
        .is_none());
}

/// A failure has no session to attach: the frame keeps exactly the shape it had,
/// so nothing downstream starts reading a key that is only ever there on the
/// success path.
#[test]
fn a_failed_result_carries_no_session() {
    let frame = result_frame("t1", &Err(RunError::Worker("boom".into())));
    assert!(frame.get("sessionId").is_none());
}
