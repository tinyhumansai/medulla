//! A non-interactive driver over the [`Runtime`] trait for scripting and
//! end-to-end automation (a docker container, a CI probe): attach a runtime,
//! submit exactly one instruction, stream the folded events to a writer as JSON
//! lines, and return once the cycle result lands.
//!
//! It exists so the core (`medulla-serve`) runtime can be exercised end-to-end
//! without a TTY or tmux — the same surface the interactive TUI drives, reduced
//! to one instruct/one cycle and a machine-readable transcript. The driver is
//! generic over [`Runtime`], so it works against any implementation (the core
//! socket in production, the in-crate stub in tests).
//!
//! ## Output contract
//!
//! One JSON object per line (NDJSON), each tagged by a `type` field:
//!
//! - `{"type":"ready","runtime":<describe>,"sessionId":<id>}` — emitted once the
//!   runtime has attached (its stream is `Live`), before the instruction is sent.
//! - `{"type":"event","seq":<u64>,"at":<i64>,"event":<TuiEvent>}` — one per
//!   folded event, in stream order, for every event produced after the submit.
//! - `{"type":"result","passCount":<i64>}` — the terminal line, emitted when the
//!   *submitted* cycle ends (correlated via the submit receipt's cycle id when
//!   the wire carries one); the driver returns immediately after.
//!
//! Errors (attach timeout, an unavailable runtime, a rejected instruction, a
//! stalled cycle) are returned as a typed [`HeadlessError`] for the caller to
//! surface on stderr and map to an exit code deterministically — they are never
//! written to the transcript stream.

use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::runtime::{Runtime, StreamState};
use crate::ui::events::TuiEvent;

/// Timeouts bounding the two waits the driver performs. Both are generous by
/// default so a slow attach or a long cycle is not cut short; callers running
/// under a test harness pass shorter values.
#[derive(Debug, Clone, Copy)]
pub struct HeadlessOptions {
    /// How long to wait for the runtime to attach (reach a `Live`/no-stream
    /// state) before giving up.
    pub ready_timeout: Duration,
    /// How long to wait for the submitted instruction's cycle to finish before
    /// giving up.
    pub cycle_timeout: Duration,
}

impl Default for HeadlessOptions {
    fn default() -> Self {
        HeadlessOptions {
            ready_timeout: Duration::from_secs(30),
            cycle_timeout: Duration::from_secs(300),
        }
    }
}

/// Why a headless run failed, as an explicit SDK-boundary error type so a
/// caller (the `medulla run` wiring, a CI harness) can map each outcome to an
/// exit code or assertion by variant instead of matching display strings.
#[derive(Debug, thiserror::Error)]
pub enum HeadlessError {
    /// The runtime never reached a `Live` (attached) stream state within
    /// [`HeadlessOptions::ready_timeout`].
    #[error("timed out waiting for the runtime to attach")]
    AttachTimeout,
    /// The runtime latched unavailable (`Stalled`) before the instruction was
    /// submitted — a rejected handshake or version mismatch.
    #[error("core runtime is unavailable: {runtime}")]
    Unavailable {
        /// The runtime's [`Runtime::describe`] line, naming what was attached.
        runtime: String,
    },
    /// The runtime latched unavailable (`Stalled`) after the instruction was
    /// accepted but before its cycle ended.
    #[error("core runtime became unavailable mid-cycle: {runtime}")]
    UnavailableMidCycle {
        /// The runtime's [`Runtime::describe`] line, naming what was attached.
        runtime: String,
    },
    /// The runtime refused the submitted instruction — no cycle will ever
    /// start, so the run fails fast instead of waiting out the cycle timeout.
    #[error("the runtime rejected the instruction: {0}")]
    SubmitRejected(#[source] anyhow::Error),
    /// The submitted instruction's cycle did not finish within
    /// [`HeadlessOptions::cycle_timeout`].
    #[error("timed out waiting for the cycle to finish")]
    CycleTimeout,
    /// Writing an NDJSON line to the caller's `out` stream failed.
    #[error("failed to write the transcript stream: {0}")]
    Output(#[from] std::io::Error),
}

/// What one headless run settled to, for the caller's exit code / assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessSummary {
    /// The cycle's pass count, from the terminal `cycle_end` event.
    pub pass_count: i64,
    /// How many event lines were streamed (excludes `ready`/`result`).
    pub events_streamed: usize,
}

/// Attach-drive-report one instruction against `runtime`, streaming the folded
/// events to `out` as NDJSON (see the module docs for the line contract).
///
/// Preconditions: `runtime` is freshly constructed (its driver may still be
/// mid-handshake — the function waits for it). Side effects: writes and flushes
/// one line per event to `out`. Errors: one [`HeadlessError`] variant per
/// failure mode — the runtime never attaches within `ready_timeout`, latches
/// unavailable, rejects the `submit`, the cycle does not finish within
/// `cycle_timeout`, or `out` fails to accept a line.
pub async fn drive_once<W: Write>(
    runtime: Arc<dyn Runtime>,
    instruction: String,
    out: &mut W,
    opts: HeadlessOptions,
) -> Result<HeadlessSummary, HeadlessError> {
    let mut rx = runtime.subscribe();

    // 1. Wait for the runtime to attach. `stream_state` gates it: `Stalled`
    //    means a fatal, latched state (a version mismatch / rejected handshake),
    //    `Live` (or `None`, for runtimes with no lossy stream) means ready, and
    //    `Resyncing` means still connecting — keep waiting.
    let ready_deadline = Instant::now() + opts.ready_timeout;
    loop {
        match runtime.stream_state() {
            Some(StreamState::Stalled) => {
                return Err(HeadlessError::Unavailable {
                    runtime: runtime.describe(),
                });
            }
            Some(StreamState::Live) | None => break,
            Some(StreamState::Resyncing) => {}
        }
        wait_for_change(&mut rx, ready_deadline)
            .await
            .map_err(|_| HeadlessError::AttachTimeout)?;
    }

    // Announce the attached runtime before the instruction goes out.
    let attached = runtime.snapshot();
    write_line(
        out,
        &json!({
            "type": "ready",
            "runtime": runtime.describe(),
            "sessionId": attached.session_id,
        }),
    )?;

    // Only stream events produced from here on: baseline at the highest seq the
    // snapshot already carries (0 on a fresh attach with no replayed history).
    let mut last_seq = attached.events.last().map(|e| e.seq).unwrap_or(0);
    // The rebaseline generation the cursor belongs to. A reconnect replay clears
    // the runtime's folded log and restarts its local seqs, so a cursor from the
    // previous generation would silently drop every replayed event — including
    // the terminal cycle_end, hanging the run until `cycle_timeout`.
    let mut replay_epoch = attached.replay_epoch;

    // 2. Submit the one instruction. A rejected `instruct` surfaces here as an
    //    `Err` (no cycle will ever start), so fail fast rather than wait it out.
    //    Keep the receipt: it names the cycle this instruction runs under.
    let receipt = runtime
        .submit_with_receipt(instruction)
        .await
        .map_err(HeadlessError::SubmitRejected)?;
    // The cycle id this run must wait for. When the wire reported one, only the
    // matching cycle_end completes the run — the attached serve may already
    // have a cycle in flight (or replay an earlier one), and completing on the
    // first observed end would cut the submitted cycle short. Without a
    // receipt (a runtime whose wire carries none) the first end still wins,
    // with the cycle timeout as the backstop either way.
    let expected_cycle: Option<String> = receipt.and_then(|r| r.cycle_id);

    // 3. Drain and stream events until the cycle ends. Completion is signalled
    //    by the terminal `cycle_end` event (rather than the `running` flag) so a
    //    cycle that folds start→end between two wakeups is never missed.
    let cycle_deadline = Instant::now() + opts.cycle_timeout;
    let mut events_streamed = 0usize;
    loop {
        let snap = runtime.snapshot();
        // The runtime rebaselined (a reconnect replay): rewind the cursor so the
        // rebuilt log folds from the top. Some replayed lines duplicate ones
        // already streamed pre-drop; that is acceptable — the replay is the
        // authoritative post-drop baseline — where a dropped terminal event is
        // not.
        if snap.replay_epoch != replay_epoch {
            replay_epoch = snap.replay_epoch;
            last_seq = 0;
        }
        let mut ended: Option<i64> = None;
        // Collect the new envelopes first so `last_seq` can be advanced inside
        // the loop without holding a borrow on `snap.events`.
        let fresh: Vec<_> = snap
            .events
            .iter()
            .filter(|e| e.seq > last_seq)
            .cloned()
            .collect();
        for env in &fresh {
            write_line(
                out,
                &json!({
                    "type": "event",
                    "seq": env.seq,
                    "at": env.at,
                    "event": env.event,
                }),
            )?;
            last_seq = env.seq;
            events_streamed += 1;
            if let TuiEvent::CycleEnd {
                cycle_id,
                pass_count,
                ..
            } = &env.event
            {
                // Only the submitted instruction's cycle terminates the run; an
                // earlier/concurrent cycle's end (still streamed above) is not
                // this run's result.
                let is_ours = expected_cycle
                    .as_deref()
                    .map(|want| want == cycle_id)
                    .unwrap_or(true);
                if is_ours {
                    ended = Some(*pass_count);
                }
            }
        }

        if let Some(pass_count) = ended {
            write_line(out, &json!({ "type": "result", "passCount": pass_count }))?;
            return Ok(HeadlessSummary {
                pass_count,
                events_streamed,
            });
        }

        // A mid-cycle drop that latches unavailable must not hang until the
        // deadline — surface it as soon as the stream reports `Stalled`.
        if runtime.stream_state() == Some(StreamState::Stalled) {
            return Err(HeadlessError::UnavailableMidCycle {
                runtime: runtime.describe(),
            });
        }

        wait_for_change(&mut rx, cycle_deadline)
            .await
            .map_err(|_| HeadlessError::CycleTimeout)?;
    }
}

/// Wait for the next change ping or until `deadline`. Returns `Err(())` only
/// when the deadline is reached; a lagged/closed channel is treated as a change
/// (the caller re-snapshots either way). A zero/elapsed remaining budget errors
/// immediately.
async fn wait_for_change(
    rx: &mut tokio::sync::broadcast::Receiver<()>,
    deadline: Instant,
) -> Result<(), ()> {
    let now = Instant::now();
    if now >= deadline {
        return Err(());
    }
    // A short poll cap keeps the loop responsive to state a bare `subscribe`
    // ping might miss (e.g. a `stream_state` transition with no fold), while the
    // outer deadline bounds the total wait.
    let remaining = deadline - now;
    let step = remaining.min(Duration::from_millis(200));
    match tokio::time::timeout(step, rx.recv()).await {
        // A ping (or a lag/closed we treat as one): re-check immediately.
        Ok(_) => Ok(()),
        // The poll cap elapsed: only a real error once the deadline is hit.
        Err(_) if Instant::now() >= deadline => Err(()),
        Err(_) => Ok(()),
    }
}

/// Write one JSON value as an NDJSON line and flush so a piped consumer (a
/// docker container reading stdout) sees each event as it lands. A serialize
/// failure folds into `io::Error` (serde_json carries the underlying I/O
/// error), which the caller surfaces as [`HeadlessError::Output`].
fn write_line<W: Write>(out: &mut W, value: &serde_json::Value) -> std::io::Result<()> {
    serde_json::to_writer(&mut *out, value).map_err(std::io::Error::from)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}
