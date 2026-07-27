//! The inbox pump — the inbound half of the runner.
//!
//! Drains the shared encrypted inbox and fans each decoded task frame out to the
//! awaiting per-dispatch [`Waiter`](super::Waiter), keyed by `correlationId`
//! (because the inbox is shared across concurrent dispatches, so one pump must
//! route each frame to the right waiter). Runs as the background task the
//! [`TaskRunner`](super::TaskRunner) spawns and aborts on drop.

use std::sync::Arc;
use std::time::Duration;

use crate::tinyplace::{
    decode_task_frame, parse_agent_capabilities, TaskFrame, TaskFrameKind, TokenUsage,
    WorkerSystemInfo,
};

use super::super::relay::Relay;
use super::super::types::{HubLog, TaskOutcome};
use super::super::ActivityLog;
use super::{CapabilitiesWaiters, SystemInfoWaiters, Waiters};

/// How many inbound messages to drain per pump tick.
const DRAIN_LIMIT: i64 = 50;

/// Route one decoded frame from `from` to its waiter, keyed by `correlationId`
/// (falling back to `taskId`). Any frame pokes the waiter's `activity` (sign of
/// life); `reply`/`error` then settle and remove it; `status` forwards; `ack`
/// just counted as activity.
///
/// A frame is only ever routed to a waiter registered for `from`. The inbox is
/// shared by every peer holding a contact edge with this identity, and a
/// correlation id is not a secret — probe ids are plain counters — so without
/// the check any contact could settle another worker's dispatch, or answer a
/// capability/system-info probe on its behalf, by guessing one. Mismatches are
/// dropped and logged rather than ignored quietly: a frame arriving under
/// someone else's correlation id is either a bug or an attempt, and both are
/// worth seeing.
pub(super) async fn route_frame(
    waiters: &Waiters,
    system_info_waiters: &SystemInfoWaiters,
    capabilities_waiters: &CapabilitiesWaiters,
    from: &str,
    frame: TaskFrame,
    log: &Option<HubLog>,
    activity: &Option<ActivityLog>,
) {
    // Checked before anything is recorded, so an impostor's frame cannot reach
    // the activity ring the Agents view renders either.
    if let Some(expected) = expected_sender(
        waiters,
        system_info_waiters,
        capabilities_waiters,
        &key_of(&frame),
    )
    .await
    {
        if expected != from {
            if let Some(log) = log {
                log(&format!(
                    "hub: dropped a {} frame for task {} — sent by {from}, which is not the worker it was dispatched to",
                    frame.kind.as_str(),
                    frame.task_id,
                ));
            }
            return;
        }
    }
    // Recorded as well as logged: the log is for a human reading afterwards,
    // this is what the Agents view renders live.
    if let Some(activity) = activity {
        // The frame's work snapshot rides along with it: this is the only
        // point where a remote worker's todo list, plan, and sub-agents enter
        // this process, and dropping it here would leave the Agents view with
        // the same one-line summary it had before.
        activity.observed_with_work(
            &frame.task_id,
            frame.kind.as_str(),
            &frame.text,
            crate::clock::now_millis(),
            frame.work.clone(),
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
    let key = key_of(&frame);
    if frame.kind == TaskFrameKind::SystemInfoResult {
        let result = serde_json::from_str::<WorkerSystemInfo>(&frame.text)
            .map_err(|error| format!("invalid worker system info: {error}"));
        if let Some(probe) = system_info_waiters.lock().await.remove(&key) {
            let _ = probe.tx.send(result);
        }
        return;
    }
    if frame.kind == TaskFrameKind::CapabilitiesResult {
        let result = parse_agent_capabilities(&frame.text)
            .ok_or_else(|| "invalid worker capabilities payload".to_string());
        if let Some(probe) = capabilities_waiters.lock().await.remove(&key) {
            let _ = probe.tx.send(result);
        }
        return;
    }
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

/// The correlation key a frame routes under: its `correlationId`, or its
/// `taskId` when it carries none.
fn key_of(frame: &TaskFrame) -> String {
    frame
        .correlation_id
        .clone()
        .unwrap_or_else(|| frame.task_id.clone())
}

/// The address the waiter registered under `key` expects to hear from, if any
/// waiter is registered at all.
///
/// `None` means nothing is waiting on this key — a late frame for a settled
/// dispatch, say — and those are left to the routing below, which finds no
/// waiter and does nothing with them beyond the activity record.
async fn expected_sender(
    waiters: &Waiters,
    system_info_waiters: &SystemInfoWaiters,
    capabilities_waiters: &CapabilitiesWaiters,
    key: &str,
) -> Option<String> {
    if let Some(waiter) = waiters.lock().await.get(key) {
        return Some(waiter.from.clone());
    }
    if let Some(probe) = system_info_waiters.lock().await.get(key) {
        return Some(probe.from.clone());
    }
    capabilities_waiters
        .lock()
        .await
        .get(key)
        .map(|probe| probe.from.clone())
}

/// The pump: drain the inbox, decode each message, route it, then sleep. Runs
/// until the owning [`TaskRunner`](super::TaskRunner) is dropped (which aborts it).
pub(super) async fn pump_loop(
    relay: Arc<dyn Relay>,
    waiters: Waiters,
    system_info_waiters: SystemInfoWaiters,
    capabilities_waiters: CapabilitiesWaiters,
    poll: Duration,
    log: Option<HubLog>,
    activity: Option<ActivityLog>,
) {
    loop {
        for msg in relay.drain_inbox(DRAIN_LIMIT).await {
            if let Some(frame) = decode_task_frame(&msg.text) {
                route_frame(
                    &waiters,
                    &system_info_waiters,
                    &capabilities_waiters,
                    &msg.from,
                    frame,
                    &log,
                    &activity,
                )
                .await;
            }
        }
        tokio::time::sleep(poll).await;
    }
}
