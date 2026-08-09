//! The idle watchdog around an in-process turn.
//!
//! # Why this exists
//!
//! Every spawned provider is supervised by a deadline that each produced event
//! pushes forward (see [`crate::daemon::providers::execute`]): a harness is
//! killed for *silence*, never for taking a long time. The embedded core used
//! to get a flat `sleep(timeout_ms)` racing the whole call instead, on the
//! reasoning that an in-process call produces no events to reset a watchdog
//! with. That reasoning was a limitation of the plumbing, not of the core: the
//! agent loop emits [`AgentProgress`] throughout a turn, and once that stream is
//! routed here (see [`super::core_contract`]) "idle" and "working" are
//! distinguishable again. A ten-minute coding turn that is producing tool calls
//! the whole time is now no longer killed at ten minutes.
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
use super::core_contract::AgentProgress;
use super::events::EventSink;

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
    tokio::pin!(call);

    let idle = Duration::from_millis(timeout_ms);
    // Armed from the start, so a turn that never emits anything is still
    // bounded — the CLI watchdog arms the same way and for the same reason.
    let mut deadline = Instant::now() + idle;
    // The core drops its sink when the turn's task-local scope ends. Once the
    // channel is closed `recv` is permanently ready with `None`, which would
    // spin this loop, so the branch retires with the sender.
    let mut open = true;

    let output = loop {
        tokio::select! {
            result = &mut call => break result,
            _ = abort.cancelled() => return Err("openhuman task aborted".to_string()),
            _ = tokio::time::sleep_until(deadline), if timeout_ms > 0 => {
                return Err(format!("openhuman task idle for {timeout_ms}ms (no events)"));
            }
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

    Ok(output)
}
