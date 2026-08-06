//! The third gate on the no-progress watchdog: a session a person is holding.
//!
//! [`liveness`](super::liveness) covers the first two — a window only accrues
//! while the link to that peer is live. This is the same idea one layer up. A
//! worker whose session an operator has taken says so
//! ([`SESSION_HELD_STATUS_PREFIX`](crate::daemon::SESSION_HELD_STATUS_PREFIX)),
//! and from that frame until the hand-back it sends nothing at all — the harness
//! is not running a turn, a human is typing in it. Thirty minutes of that is
//! indistinguishable from a crashed worker to a clock that only counts frames,
//! and the old answer was to reap the dispatch: the task died while the person
//! was still working, and the only notice anyone got was `bridge task timed out`.
//!
//! The three tests here are deliberately a set, because any one alone would pass
//! for the wrong reason:
//!
//! - [`a_held_session_outlasts_the_no_progress_window`] proves the clock pauses.
//! - [`a_silent_worker_that_never_reported_a_hold_still_times_out`] proves it was
//!   *gated*, not deleted — the same test would pass if the watchdog had simply
//!   been removed.
//! - [`a_worker_that_dies_after_the_hand_back_is_still_given_up_on`] proves the
//!   pause ends where it should. A hold that leaked past the hand-back would
//!   make a dead worker unreapable for ever, which is exactly the leak the
//!   window exists to prevent.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::bridge::{BridgeLiveness, InboundMessage};
use crate::hub::{Relay, RunError, TaskRequest, TaskRunner};
use crate::protocol::{decode_task_frame, encode_task_frame, EncodeFrameInput, TaskFrameKind};

/// Comfortably longer than the runner's 240 s no-progress window, and about as
/// long as a person actually keeps a session: the point is the ratio.
const HOLD: Duration = Duration::from_secs(1_800);

/// Longer than the window too, so a resumed clock has room to fire.
const AFTER_HAND_BACK: Duration = Duration::from_secs(600);

/// A worker that acks, then goes quiet in the way a held session does.
struct HoldingPeer {
    inbox: Mutex<VecDeque<InboundMessage>>,
    /// The correlation id of the dispatch, learned from the frame we were sent.
    correlation: Mutex<Option<String>>,
    /// Whether to announce the hold at all. `false` is a worker that simply
    /// stopped talking — a crash, which must still be reaped.
    announces: bool,
}

impl HoldingPeer {
    fn new(announces: bool) -> Arc<Self> {
        Arc::new(HoldingPeer {
            inbox: Mutex::new(VecDeque::new()),
            correlation: Mutex::new(None),
            announces,
        })
    }

    /// Queue one frame from the worker, under the dispatch's correlation id.
    async fn emit(&self, kind: TaskFrameKind, text: &str) {
        let correlation = self.correlation.lock().await.clone();
        let body = encode_task_frame(EncodeFrameInput {
            transport: None,
            kind,
            task_id: "t1".to_string(),
            text: text.to_string(),
            ts: crate::clock::iso_now(),
            correlation_id: correlation,
            harness: None,
            provider: None,
            custom_harness: None,
            model: None,
            tool_mode: None,
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: None,
            fleet_depth: 0,
        });
        self.inbox.lock().await.push_back(InboundMessage {
            from: "host-a".to_string(),
            text: body,
        });
    }

    /// The status frame the worker sends when an operator takes the session.
    async fn report_held(&self) {
        self.emit(
            TaskFrameKind::Status,
            &format!(
                "{} · the codex turn is suspended, not lost",
                crate::daemon::SESSION_HELD_STATUS_PREFIX
            ),
        )
        .await;
    }

    /// The status frame that ends the hold and restarts the hand-back turn.
    async fn report_resumed(&self) {
        self.emit(
            TaskFrameKind::Status,
            &format!(
                "{} · reviewing what changed",
                crate::daemon::SESSION_RESUMED_STATUS_PREFIX
            ),
        )
        .await;
    }

    /// The hand-back turn's answer — the task's result.
    async fn reply(&self) {
        self.emit(TaskFrameKind::Reply, "finished after the hand-back")
            .await;
    }
}

#[async_trait]
impl Relay for HoldingPeer {
    async fn send(&self, _to: &str, body: &str) -> Result<(), String> {
        let Some(frame) = decode_task_frame(body) else {
            return Ok(());
        };
        if frame.kind != TaskFrameKind::Task {
            return Ok(());
        }
        *self.correlation.lock().await = frame.correlation_id.clone();
        // Alive, and working — then a person takes the session and the frames
        // stop.
        self.emit(TaskFrameKind::Ack, "task accepted").await;
        if self.announces {
            self.report_held().await;
        }
        Ok(())
    }

    async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage> {
        if limit <= 0 {
            return Vec::new();
        }
        let mut inbox = self.inbox.lock().await;
        let count = usize::try_from(limit)
            .unwrap_or(usize::MAX)
            .min(inbox.len());
        inbox.drain(..count).collect()
    }

    async fn request_contact(&self, _peer: &str) -> Result<(), String> {
        Ok(())
    }

    async fn contact_accepted(&self, _peer: &str) -> bool {
        true
    }

    async fn reset_session(&self, _peer: &str) {}

    async fn liveness(&self, _peer: &str) -> BridgeLiveness {
        BridgeLiveness::Live
    }
}

/// The dispatch every test here runs.
fn req() -> TaskRequest {
    TaskRequest {
        transport: None,
        task_id: "t1".to_string(),
        abort_id: "t1".to_string(),
        cycle_id: Some("c1".to_string()),
        instruction: "finish the migration".to_string(),
        worker_address: "host-a".to_string(),
        provider: None,
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    }
}

#[tokio::test(start_paused = true)]
async fn a_held_session_outlasts_the_no_progress_window() {
    // Half an hour of a person working in the session, against a 240-second
    // window. Ungated, this dispatch is reaped ~28 minutes before its result
    // exists — and reaped is not merely "late": the runner sends the worker an
    // abort, so the hand-back turn would never even run.
    let peer = HoldingPeer::new(true);
    let runner = TaskRunner::start(peer.clone() as Arc<dyn Relay>, Duration::from_millis(10));

    let operator = {
        let peer = peer.clone();
        tokio::spawn(async move {
            tokio::time::sleep(HOLD).await;
            peer.report_resumed().await;
            peer.reply().await;
        })
    };

    let outcome = runner.run(req(), None).await;
    operator.await.expect("the operator task completes");

    let outcome = outcome.expect("a held session must not fail its task");
    assert_eq!(outcome.reply, "finished after the hand-back");
}

#[tokio::test(start_paused = true)]
async fn a_silent_worker_that_never_reported_a_hold_still_times_out() {
    // The gate is a gate. This peer acks and then dies, saying nothing about
    // control — the exact case the window exists for, and the one a deleted
    // window would leave pinned for ever.
    let peer = HoldingPeer::new(false);
    let runner = TaskRunner::start(peer as Arc<dyn Relay>, Duration::from_millis(10));

    let outcome = runner.run(req(), None).await;

    assert!(
        matches!(outcome, Err(RunError::Timeout)),
        "a worker that acked and vanished must still be given up on: {outcome:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_worker_that_dies_after_the_hand_back_is_still_given_up_on() {
    // The pause ends with the hold. A worker that reports the hand-back and then
    // stops talking is a crashed worker again, and must be reaped on the same
    // schedule as any other — otherwise one hold makes a dispatch immortal.
    let peer = HoldingPeer::new(true);
    let runner = TaskRunner::start(peer.clone() as Arc<dyn Relay>, Duration::from_millis(10));

    let operator = {
        let peer = peer.clone();
        tokio::spawn(async move {
            tokio::time::sleep(HOLD).await;
            // Handed back — and then nothing. No reply ever comes.
            peer.report_resumed().await;
            tokio::time::sleep(AFTER_HAND_BACK).await;
        })
    };

    let outcome = runner.run(req(), None).await;
    operator.await.expect("the operator task completes");

    assert!(
        matches!(outcome, Err(RunError::Timeout)),
        "the window must resume when the hold ends: {outcome:?}"
    );
}
