//! The inbox pump — the inbound half of the runner.
//!
//! Drains the shared encrypted inbox and fans each decoded task frame out to the
//! awaiting per-dispatch [`Waiter`](super::Waiter), keyed by `correlationId`
//! (because the inbox is shared across concurrent dispatches, so one pump must
//! route each frame to the right waiter). Runs as the background task the
//! [`TaskRunner`](super::TaskRunner) spawns and aborts on drop.

use std::sync::Arc;
use std::time::Duration;

use crate::tinyplace::{decode_task_frame, TaskFrame, TaskFrameKind, TokenUsage};

use super::super::relay::Relay;
use super::super::types::{HubLog, TaskOutcome};
use super::super::ActivityLog;
use super::Waiters;

/// How many inbound messages to drain per pump tick.
const DRAIN_LIMIT: i64 = 50;

/// Route one decoded frame to its waiter, keyed by `correlationId` (falling back
/// to `taskId`). Any frame pokes the waiter's `activity` (sign of life);
/// `reply`/`error` then settle and remove it; `status` forwards; `ack` just
/// counted as activity.
pub(super) async fn route_frame(
    waiters: &Waiters,
    frame: TaskFrame,
    log: &Option<HubLog>,
    activity: &Option<ActivityLog>,
) {
    // Recorded as well as logged: the log is for a human reading afterwards,
    // this is what the Agents view renders live.
    if let Some(activity) = activity {
        activity.observed(
            &frame.task_id,
            frame.kind.as_str(),
            &frame.text,
            crate::clock::now_millis(),
        );
    }
    // Every frame a worker sends, as it arrives. The hub used to report only the
    // settled outcome, so a reply that never came and a reply that came back
    // empty read the same from here — and neither said whether the worker had
    // been talking at all.
    if let Some(log) = log {
        log(&format!(
            "hub ← task {} {} · {} chars: {}",
            frame.task_id,
            frame.kind.as_str(),
            frame.text.chars().count(),
            crate::logging::preview(&frame.text),
        ));
    }
    let key = frame
        .correlation_id
        .clone()
        .unwrap_or_else(|| frame.task_id.clone());
    // One lock for the whole routing — every op below is synchronous.
    let mut map = waiters.lock().await;
    if let Some(w) = map.get(&key) {
        w.activity.notify_one();
    }
    match frame.kind {
        TaskFrameKind::Reply => {
            if let Some(w) = map.remove(&key) {
                let _ = w.reply.send(Ok(TaskOutcome {
                    reply: frame.text,
                    usage: frame.usage.unwrap_or(TokenUsage {
                        input_tokens: 0,
                        output_tokens: 0,
                    }),
                    harness: frame.harness,
                }));
            }
        }
        TaskFrameKind::Error => {
            if let Some(w) = map.remove(&key) {
                let _ = w.reply.send(Err(frame.text));
            }
        }
        TaskFrameKind::Status => {
            if let Some(w) = map.get(&key) {
                if let Some(tx) = &w.status {
                    let _ = tx.send(frame.text);
                }
            }
        }
        // ack / task / input / capabilities* — activity already recorded.
        _ => {}
    }
}

/// The pump: drain the inbox, decode each message, route it, then sleep. Runs
/// until the owning [`TaskRunner`](super::TaskRunner) is dropped (which aborts it).
pub(super) async fn pump_loop(
    relay: Arc<dyn Relay>,
    waiters: Waiters,
    poll: Duration,
    log: Option<HubLog>,
    activity: Option<ActivityLog>,
) {
    loop {
        for msg in relay.drain_inbox(DRAIN_LIMIT).await {
            if let Some(frame) = decode_task_frame(&msg.text) {
                route_frame(&waiters, frame, &log, &activity).await;
            }
        }
        tokio::time::sleep(poll).await;
    }
}
