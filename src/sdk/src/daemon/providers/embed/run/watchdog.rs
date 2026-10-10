//! The idle watchdog around an in-process turn.
//!
//! # Why this exists
//!
//! Every spawned provider is supervised by a deadline that each produced event
//! pushes forward (see [`crate::daemon::providers::execute`]): a harness is
//! killed for *silence*, never for taking a long time. An in-process turn once
//! got a flat `sleep(timeout_ms)` racing the whole call instead, on the
//! reasoning that it produces no events to reset a watchdog with. That was a
//! limitation of the plumbing, not of the harness: the agent loop emits an
//! [`AgentProgress`] stream throughout a turn, and with that routed here "idle"
//! and "working" are distinguishable. A ten-minute coding turn producing tool
//! calls the whole time is not killed at ten minutes.
//!
//! # Shape
//!
//! Deliberately generic over the future: the watchdog has nothing to do with
//! what the core call returns, and keeping it ignorant is what lets its
//! behaviour be tested without booting a core.

use std::future::Future;
use std::time::Duration;

use tokio::sync::mpsc::Receiver;
use tokio::time::Instant;

use super::super::super::types::Abort;
use super::EventSink;
use openhuman_embed::agent_progress::AgentProgress;

/// Drive `call` to completion under an idle watchdog.
///
/// Each event arriving on `progress` is folded into `sink` and pushes the idle
/// deadline out by `timeout_ms`; `timeout_ms` of 0 means the operator set no
/// ceiling and the call runs until it finishes or is aborted. Events still
/// queued when `call` resolves are drained before returning, so a turn that
/// finishes fast does not lose the tail of its own transcript.
///
/// # Errors
///
/// Returns the abort sentence when `abort` fires, and the idle sentence after
/// `timeout_ms` of genuine silence — no progress event and no completion.
#[cfg(test)]
pub(super) async fn drive<F>(
    call: F,
    progress: &mut Receiver<AgentProgress>,
    abort: &Abort,
    timeout_ms: u64,
    sink: &mut EventSink,
) -> Result<F::Output, String>
where
    F: Future,
{
    let (_events_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    drive_events(call, progress, &mut events, abort, timeout_ms, sink).await
}

/// Drive a turn and its inline approval events through one semantic sink.
pub(super) async fn drive_events<F>(
    call: F,
    progress: &mut Receiver<AgentProgress>,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<(String, serde_json::Value)>,
    abort: &Abort,
    timeout_ms: u64,
    sink: &mut EventSink,
) -> Result<F::Output, String>
where
    F: Future,
{
    tokio::pin!(call);

    let idle = Duration::from_millis(timeout_ms);
    // Armed from the start, so a turn that never emits anything is still
    // bounded — the CLI watchdog arms the same way and for the same reason.
    let mut deadline = Instant::now() + idle;
    // The forwarder drops its sender when the turn ends. Once the channel is
    // closed `recv` is permanently ready with `None`, which would spin this
    // loop, so the branch retires with the sender.
    let mut open = true;
    let mut events_open = true;

    let output = loop {
        tokio::select! {
            result = &mut call => break result,
            _ = abort.cancelled() => return Err("local task aborted".to_string()),
            _ = tokio::time::sleep_until(deadline), if timeout_ms > 0 => {
                return Err(format!("local task idle for {timeout_ms}ms (no events)"));
            }
            event = events.recv(), if events_open => match event {
                Some((kind, payload)) => { sink.emit(&kind, payload); deadline = Instant::now() + idle; }
                None => events_open = false,
            },
            received = progress.recv(), if open => match received {
                Some(event) => {
                    sink.emit_progress(&event);
                    // Reset on *every* event, including the ones that fold into
                    // no transcript frame: a cost rollup is still proof the turn
                    // is alive, which is the only question this deadline asks.
                    deadline = Instant::now() + idle;
                }
                None => open = false,
            },
        }
    };

    // The call resolving does not empty the channel: the last tool result and
    // the closing text deltas are typically still queued behind it.
    while let Ok(event) = progress.try_recv() {
        sink.emit_progress(&event);
    }

    while let Ok((kind, payload)) = events.try_recv() {
        sink.emit(&kind, payload);
    }
    Ok(output)
}
