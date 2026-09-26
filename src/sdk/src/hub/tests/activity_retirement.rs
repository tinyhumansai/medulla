//! Tests for the hub's [`ActivityLog`] retiring work that has stopped.
//!
//! The log is what the Sessions rail draws a row from, so a task the log keeps
//! is a row on the operator's screen. Two rules make that list stay honest, and
//! both were missing:
//!
//! - a task that **settled** leaves once its retention window passes, whatever
//!   ended it — a reply, a script step exiting non-zero, an agent step's worker
//!   error, or a cancellation. An operator who ran dispatches all afternoon was
//!   otherwise left reading a dozen finished rows to find the live one;
//! - a task that produced **no frame of its own** is settled anyway by
//!   [`ActivityLog::record_outcome`]. A dispatch reaped for going silent, one
//!   whose bridge dropped, or one an operator aborted generates its outcome on
//!   this side, reaches no worker, and used to leave a row reading `running`
//!   for the life of the process.
//!
//! A task with no terminal frame is never retired, however old: it may still be
//! executing, and its row is what the operator cancels it from.
//!
//! Every assertion supplies its own `now` to the retirement pass, so none of
//! this waits on wall time. The frames are stamped relative to the real clock
//! rather than to a literal, because [`ActivityLog::record_outcome`] stamps its
//! own frame with the real clock and a fixed base would drift past it.

use super::super::activity::ActivityLog;
use super::super::{RunError, TaskOutcome};
use crate::protocol::TokenUsage;

/// The retention window, mirroring the private `SETTLED_RETENTION_MS`.
const SETTLED_RETENTION_MS: i64 = 2 * 60 * 1_000;

/// The reference instant these tests place their frames around.
fn base() -> i64 {
    crate::clock::now_millis()
}

/// A successful outcome carrying `reply`.
fn done(reply: &str) -> Result<TaskOutcome, RunError> {
    Ok(TaskOutcome {
        reply: reply.to_string(),
        usage: TokenUsage {
            input_tokens: 0,
            output_tokens: 0,
        },
        harness: None,
        session_id: None,
        transcript: Vec::new(),
    })
}

/// The distinct task ids the log still holds as of `now`.
fn task_ids(log: &ActivityLog, now: i64) -> Vec<String> {
    let mut ids: Vec<String> = log
        .snapshot_at(now)
        .into_iter()
        .map(|entry| entry.task_id)
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

#[test]
fn a_task_that_replied_leaves_the_log_once_its_window_passes() {
    let now = base();
    let log = ActivityLog::new();
    log.dispatched_as("wire-1", "mcp:pty-1/t:mcp-1", "worker-a");
    log.observed("wire-1", "status", "reading files", now - 10_000);
    log.observed("wire-1", "reply", "done", now - 5_000);

    assert_eq!(
        task_ids(&log, now),
        vec!["mcp:pty-1/t:mcp-1".to_string()],
        "a task that has only just settled is still worth showing"
    );

    assert!(
        task_ids(&log, now + SETTLED_RETENTION_MS).is_empty(),
        "a settled task leaves the log — and with it the rail — once its window passes"
    );
}

#[test]
fn a_run_that_fails_at_a_script_step_leaves_no_row_behind() {
    // A script node exiting non-zero comes back as a worker error, which the
    // pump records as a terminal `error` frame. The row must retire exactly as a
    // successful one does: the operator asked for finished work to be gone, not
    // for successful work to be gone.
    let now = base();
    let log = ActivityLog::new();
    log.dispatched_as("wire-1", "mcp:pty-1/t:mcp-1", "worker-a");
    log.observed("wire-1", "status", "running prepare", now - 20_000);
    log.observed("wire-1", "error", "bash exited with 128", now - 9_000);

    assert!(
        log.running_by_agent().is_empty(),
        "a failed script step is not still running"
    );
    assert!(
        task_ids(&log, now + SETTLED_RETENTION_MS).is_empty(),
        "and it must not pin a row forever"
    );
}

#[test]
fn a_run_that_fails_at_an_agent_step_leaves_no_row_behind() {
    // The agent-node failure the operator actually hit: the harness answered the
    // worker with an upstream gateway error rather than a reply. It arrives as
    // an outcome on this side, with no frame of its own — so nothing settles the
    // row unless `record_outcome` does.
    let now = base();
    let log = ActivityLog::new();
    log.dispatched_as("wire-1", "mcp:pty-1/t:mcp-1", "worker-a");
    log.observed("wire-1", "ack", "", now - 30_000);

    log.record_outcome(
        "wire-1",
        &Err(RunError::Worker(
            "claude ACP error: Internal error: API Error: 400".to_string(),
        )),
    );

    assert!(
        log.running_by_agent().is_empty(),
        "an agent step that errored is not still running"
    );
    let error = log
        .snapshot_at(now)
        .into_iter()
        .find(|entry| entry.kind == "error")
        .expect("the failure is recorded where the operator reads it");
    assert_eq!(error.task_id, "mcp:pty-1/t:mcp-1");
    assert!(error.content.contains("API Error: 400"));

    assert!(
        task_ids(&log, now + 2 * SETTLED_RETENTION_MS).is_empty(),
        "and then it leaves"
    );
}

#[test]
fn a_cancelled_dispatch_leaves_no_row_behind() {
    // Cancellation is the case where the operator is *most* sure the work has
    // stopped, and it produces no worker frame at all: the runner returns
    // `Aborted` locally.
    let now = base();
    let log = ActivityLog::new();
    log.dispatched_as("wire-1", "mcp:pty-1/t:mcp-1", "worker-a");
    log.observed("wire-1", "status", "working", now - 30_000);

    log.record_outcome("wire-1", &Err(RunError::Aborted));

    assert!(
        log.running_by_agent().is_empty(),
        "a cancelled dispatch stops counting as running the moment it is cancelled"
    );
    assert!(task_ids(&log, now + 2 * SETTLED_RETENTION_MS).is_empty());
}

#[test]
fn a_running_task_is_never_retired_however_old() {
    let now = base();
    let log = ActivityLog::new();
    log.dispatched_as("wire-1", "mcp:pty-1/t:mcp-1", "worker-a");
    log.observed("wire-1", "status", "still going", now - 86_400_000);

    assert_eq!(
        task_ids(&log, now),
        vec!["mcp:pty-1/t:mcp-1".to_string()],
        "a task with no terminal frame may still be executing; hiding it would \
         hide live work and the row that cancels it"
    );
}

#[test]
fn retiring_a_task_forgets_its_attribution_too() {
    // Otherwise the attribution map — bounded separately, and never rewritten —
    // outlives every task it describes and becomes the leak the entries ring
    // stopped being.
    let now = base();
    let log = ActivityLog::new();
    log.dispatched_as("wire-1", "mcp:pty-1/t:mcp-1", "worker-a");
    log.observed("wire-1", "reply", "done", now);

    log.retire_settled(now + SETTLED_RETENTION_MS);

    log.observed("wire-1", "status", "a stray late frame", now);
    let entry = &log.snapshot_at(now)[0];
    assert_eq!(
        entry.task_id, "wire-1",
        "with the attribution gone the wire id no longer resolves to the retired alias"
    );
    assert_eq!(entry.agent_id, "");
}

#[test]
fn the_fallback_never_settles_a_task_twice() {
    let now = base();
    let log = ActivityLog::new();
    log.dispatched_as("wire-1", "mcp:pty-1/t:mcp-1", "worker-a");
    log.observed("wire-1", "reply", "the worker's own answer", now);

    log.record_outcome("wire-1", &done("a second account of the same work"));

    let terminal: Vec<_> = log
        .snapshot_at(now)
        .into_iter()
        .filter(|entry| matches!(entry.kind.as_str(), "reply" | "error"))
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].content, "the worker's own answer");
}

#[test]
fn reading_the_log_retires_what_has_aged_out() {
    // The rail reads `snapshot` every frame and nothing else drives retirement
    // on a host that has gone quiet — which is precisely the state the operator
    // was in when the stale rows would not go away.
    let log = ActivityLog::new();
    log.dispatched_as("wire-1", "mcp:pty-1/t:mcp-1", "worker-a");
    log.observed(
        "wire-1",
        "reply",
        "done",
        base() - SETTLED_RETENTION_MS - 1_000,
    );

    assert!(
        log.snapshot().is_empty(),
        "a settled task aged past its window is gone by the time anyone reads the log"
    );
}
