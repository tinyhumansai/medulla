//! The two-layer timeout gate — host-link protocol §6.3.
//!
//! This is the single most important integration detail of the host link, and
//! the reason it gets its own file. `TaskRunner` owns `ACK_WINDOW` (12 s) and
//! `IDLE_WINDOW` (240 s). Both exist because the old mailbox transport could
//! silently black-hole a frame; the link cannot, because it owns retransmission
//! and recovers an outage by itself.
//!
//! So both clocks are paused while the link is not `Live`. The pair of tests
//! here is deliberately two-sided, because either half alone would pass for the
//! wrong reason:
//!
//! - [`an_outage_longer_than_the_ack_window_does_not_fail_the_task`] proves the
//!   clock pauses. Without the gate a 30-second blip kills a task the transport
//!   was in the middle of recovering, which would forfeit the entire reason for
//!   adopting the link.
//! - [`a_hung_peer_on_a_live_link_still_times_out`] proves the clock was
//!   *gated*, not disabled. A test that only checked the first would also pass
//!   if the timeout had simply been deleted.
//!
//! [`a_dead_peer_does_not_pause_another_peers_clock`] covers the third clause of
//! §6.3: the gate is per *peer*, not per link.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::bridge::{BridgeLiveness, InboundMessage};
use crate::hub::{Relay, RunError, TaskRequest, TaskRunner};
use crate::protocol::{decode_task_frame, encode_task_frame, EncodeFrameInput, TaskFrameKind};

/// A short ack window, so a test drives the reset-and-resend path without real
/// delays. The gate's behaviour is a ratio, not an absolute: what matters is
/// that the outage is many times this.
const ACK_WINDOW: Duration = Duration::from_millis(200);

/// How long the link stays down. Ten times the ack window: comfortably enough
/// that an ungated clock would have expired and burned both resets.
const OUTAGE: Duration = Duration::from_millis(2_000);

/// A peer behind a link whose liveness the test drives.
struct GatedPeer {
    /// What [`Relay::liveness`] reports right now.
    liveness: Mutex<BridgeLiveness>,
    inbox: Mutex<VecDeque<InboundMessage>>,
    /// Whether the peer ever answers. `false` is a genuinely hung worker.
    answers: bool,
    sends: AtomicU32,
}

impl GatedPeer {
    fn new(liveness: BridgeLiveness, answers: bool) -> Arc<Self> {
        Arc::new(GatedPeer {
            liveness: Mutex::new(liveness),
            inbox: Mutex::new(VecDeque::new()),
            answers,
            sends: AtomicU32::new(0),
        })
    }

    /// Bring the link back up.
    async fn recover(&self) {
        *self.liveness.lock().await = BridgeLiveness::Live;
    }

    /// How many task frames the runner has sent — one per attempt, so this is
    /// the reset-and-resend count plus one.
    fn sends(&self) -> u32 {
        self.sends.load(Ordering::Relaxed)
    }

    /// Queue the peer's terminal `reply` for a task frame.
    async fn answer(&self, body: &str) {
        let Some(frame) = decode_task_frame(body) else {
            return;
        };
        if frame.kind != TaskFrameKind::Task {
            return;
        }
        let reply = encode_task_frame(EncodeFrameInput {
            transport: None,
            kind: TaskFrameKind::Reply,
            task_id: frame.task_id.clone(),
            text: "done".to_string(),
            ts: crate::clock::iso_now(),
            correlation_id: frame.correlation_id.clone(),
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
            text: reply,
        });
    }
}

#[async_trait]
impl Relay for GatedPeer {
    async fn send(&self, _to: &str, body: &str) -> Result<(), String> {
        self.sends.fetch_add(1, Ordering::Relaxed);
        // The peer only answers once the link is actually carrying: an offline
        // link delivers nothing, which is the whole situation under test.
        if self.answers && *self.liveness.lock().await == BridgeLiveness::Live {
            self.answer(body).await;
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
        *self.liveness.lock().await
    }
}

/// The dispatch both tests run.
fn req() -> TaskRequest {
    TaskRequest {
        transport: None,
        task_id: "t1".to_string(),
        abort_id: "t1".to_string(),
        cycle_id: Some("c1".to_string()),
        instruction: "do the thing".to_string(),
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
async fn an_outage_longer_than_the_ack_window_does_not_fail_the_task() {
    // The link is down when the task is dispatched and stays down for ten ack
    // windows. An ungated clock would expire, reset and resend twice, and return
    // `Timeout` — failing a task the transport was about to carry fine.
    let peer = GatedPeer::new(BridgeLiveness::Offline, true);
    let runner = TaskRunner::start_with_ack_window(
        peer.clone() as Arc<dyn Relay>,
        Duration::from_millis(10),
        ACK_WINDOW,
    );

    let recovering = {
        let peer = peer.clone();
        tokio::spawn(async move {
            tokio::time::sleep(OUTAGE).await;
            peer.recover().await;
            // The frame the runner sent while the link was down is still in the
            // outbound state: recovery delivers it, it is not re-sent.
            let dispatched = encode_task_frame(EncodeFrameInput {
                transport: None,
                kind: TaskFrameKind::Task,
                task_id: "t1".to_string(),
                text: "do the thing".to_string(),
                ts: crate::clock::iso_now(),
                correlation_id: Some("c1/t1/0".to_string()),
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
            peer.answer(&dispatched).await;
        })
    };

    let outcome = runner.run(req(), None).await;
    recovering.await.expect("the recovery task completes");

    assert!(
        outcome.is_ok(),
        "an outage the link recovers from must not fail the task: {outcome:?}"
    );
    assert_eq!(
        peer.sends(),
        1,
        "the paused clock must not have burned a reset-and-resend attempt"
    );
}

#[tokio::test(start_paused = true)]
async fn a_hung_peer_on_a_live_link_still_times_out() {
    // The counterpart, and the reason the gate is a gate rather than a deletion:
    // the link is healthy the whole time and the *peer* is the thing that never
    // answers. The ack window must still fire.
    let peer = GatedPeer::new(BridgeLiveness::Live, false);
    let runner = TaskRunner::start_with_ack_window(
        peer.clone() as Arc<dyn Relay>,
        Duration::from_millis(10),
        ACK_WINDOW,
    );

    let outcome = runner.run(req(), None).await;

    assert!(
        matches!(outcome, Err(RunError::Timeout)),
        "a hung peer on a live link must still time out: {outcome:?}"
    );
    // One initial send, `MAX_RESETS` reset-and-resend attempts, and the
    // best-effort `abort` frame that tells the worker to stop.
    assert_eq!(peer.sends(), 4, "every reset attempt ran, then the abort");
}

#[tokio::test(start_paused = true)]
async fn a_degraded_link_pauses_the_clock_just_as_an_offline_one_does() {
    // `Degraded` is not a lesser outage to be timed out slightly more patiently:
    // §6.2 makes all three states advisory and only `Live` means the peer's
    // silence is the peer's own doing.
    let peer = GatedPeer::new(BridgeLiveness::Degraded, true);
    let runner = TaskRunner::start_with_ack_window(
        peer.clone() as Arc<dyn Relay>,
        Duration::from_millis(10),
        ACK_WINDOW,
    );

    let recovering = {
        let peer = peer.clone();
        tokio::spawn(async move {
            tokio::time::sleep(OUTAGE).await;
            peer.recover().await;
            let dispatched = encode_task_frame(EncodeFrameInput {
                transport: None,
                kind: TaskFrameKind::Task,
                task_id: "t1".to_string(),
                text: "do the thing".to_string(),
                ts: crate::clock::iso_now(),
                correlation_id: Some("c1/t1/0".to_string()),
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
            peer.answer(&dispatched).await;
        })
    };

    let outcome = runner.run(req(), None).await;
    recovering.await.expect("the recovery task completes");

    assert!(outcome.is_ok(), "degraded is not a failure: {outcome:?}");
    assert_eq!(peer.sends(), 1);
}

/// A fleet whose liveness answer depends on *which* peer is asked.
///
/// Every peer is hung; only one of them is offline. This is what separates a
/// per-peer gate from a per-link one.
struct Fleet {
    /// The one address that reports offline.
    offline: &'static str,
    sends: AtomicU32,
}

#[async_trait]
impl Relay for Fleet {
    async fn send(&self, _to: &str, _body: &str) -> Result<(), String> {
        self.sends.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn drain_inbox(&self, _limit: i64) -> Vec<InboundMessage> {
        Vec::new()
    }

    async fn request_contact(&self, _peer: &str) -> Result<(), String> {
        Ok(())
    }

    async fn contact_accepted(&self, _peer: &str) -> bool {
        true
    }

    async fn reset_session(&self, _peer: &str) {}

    async fn liveness(&self, peer: &str) -> BridgeLiveness {
        if peer == self.offline {
            BridgeLiveness::Offline
        } else {
            BridgeLiveness::Live
        }
    }
}

/// A dispatch to `address` on the shared fleet relay.
fn req_to(address: &str) -> TaskRequest {
    TaskRequest {
        worker_address: address.to_string(),
        ..req()
    }
}

#[tokio::test(start_paused = true)]
async fn a_dead_peer_does_not_pause_another_peers_clock() {
    // §6.3: "The gate is per peer, not per link." An orchestrator holds sessions
    // with many hosts; gating on an aggregate would let one laptop going to
    // sleep stop every other host's clock, so a task dispatched to a healthy
    // worker would stop timing out for an unrelated reason.
    let fleet = Arc::new(Fleet {
        offline: "host-asleep",
        sends: AtomicU32::new(0),
    });
    let runner = TaskRunner::start_with_ack_window(
        fleet.clone() as Arc<dyn Relay>,
        Duration::from_millis(10),
        ACK_WINDOW,
    );

    // The healthy-but-hung peer must still time out, even though a sibling peer
    // on the same relay is offline the whole time.
    let outcome = runner.run(req_to("host-awake"), None).await;
    assert!(
        matches!(outcome, Err(RunError::Timeout)),
        "a hung peer on its own live link must time out however dead its siblings are: {outcome:?}"
    );

    // And the offline one is still paused: it does not settle within the window
    // the awake peer just spent in full.
    let paused =
        tokio::time::timeout(ACK_WINDOW * 8, runner.run(req_to("host-asleep"), None)).await;
    assert!(
        paused.is_err(),
        "the offline peer's clock must still be paused: {paused:?}"
    );
}
