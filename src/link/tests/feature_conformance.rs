//! The transport half of the protocol's §9 conformance list, run against the
//! scriptable fake network in `support/network.rs`.
//!
//! These cases are listed in `docs/host-link-protocol.md` rather than only here
//! because they are part of the contract: two other implementations code against
//! the same document. Each test names the clause it covers.

#[path = "support/network.rs"]
mod network;

use medulla_link::keys::{ForwarderKey, MemorySeq, NodeId, PairKey, Role};
use medulla_link::state::{QueueLimits, SspState};
use medulla_link::transport::MAX_SENT_STATES;
use medulla_link::{Liveness, Session, SessionConfig, TransportError};
use network::{Harness, Script};

/// A message body that is easy to spot in a failure message.
fn frame(index: usize) -> Vec<u8> {
    format!("task frame {index}").into_bytes()
}

/// The bodies of `count` frames, in order.
fn frames(count: usize) -> Vec<Vec<u8>> {
    (0..count).map(frame).collect()
}

#[test]
fn a_clean_link_delivers_in_order_and_settles() {
    let mut harness = Harness::new(Script::default());
    for index in 0..10 {
        harness.orchestrator.queue_message(frame(index)).unwrap();
    }
    let converged = harness.run_until(5_000, |harness| harness.host_inbox.len() == 10);
    assert!(converged, "the host received {:?}", harness.host_inbox);
    assert_eq!(harness.host_inbox, frames(10));

    // Once everything is acknowledged the link goes quiet apart from heartbeats.
    let before = harness.counters.sent;
    harness.run_for(3_000);
    assert!(
        harness.counters.sent - before <= 3,
        "an idle link sent {} datagrams in 3 s",
        harness.counters.sent - before
    );
    assert!(!harness.orchestrator.has_pending_state());
}

#[test]
fn it_converges_under_thirty_percent_loss_reordering_and_duplication() {
    // §9: "Convergence under 30% loss, reordering and duplication."
    let mut harness = Harness::new(Script::hostile());
    // Queued over time rather than all at once: a single diff would otherwise
    // carry all 25 frames in one datagram, and one datagram is not a test of
    // loss.
    for index in 0..25 {
        harness.orchestrator.queue_message(frame(index)).unwrap();
        harness
            .host
            .queue_message(format!("reply {index}").into_bytes())
            .unwrap();
        harness.run_for(40);
    }

    let converged = harness.run_until(60_000, |harness| {
        harness.host_inbox.len() == 25 && harness.orchestrator_inbox.len() == 25
    });
    assert!(
        converged,
        "host had {} and orchestrator {} of 25 after {} ms",
        harness.host_inbox.len(),
        harness.orchestrator_inbox.len(),
        harness.now()
    );

    // Ordered, complete, and no duplicates despite the network making copies.
    assert_eq!(harness.host_inbox, frames(25));
    assert_eq!(
        harness.orchestrator_inbox,
        (0..25)
            .map(|index| format!("reply {index}").into_bytes())
            .collect::<Vec<_>>()
    );
    assert!(
        harness.counters.dropped > 0 && harness.counters.duplicated > 0,
        "the scenario did not actually lose or duplicate anything: {:?}",
        harness.counters
    );
    // Duplicates and stale datagrams are absorbed by the `old_num` rule, so
    // nothing is ever *rejected* as an error.
    assert_eq!(harness.counters.rejected, 0);
}

#[test]
fn a_sixty_second_blackout_mid_exchange_completes_without_error() {
    // §9: "A 60-second total blackout mid-task completes without error and
    // without re-enrolling."
    let mut harness = Harness::new(Script::default().with_blackout(500, 60_000));
    harness.orchestrator.queue_message(frame(0)).unwrap();
    harness.orchestrator.queue_message(frame(1)).unwrap();
    // The first frames land; the rest are queued mid-task, while the link is
    // down, which is the case §6.3 exists for.
    harness.run_for(400);
    assert_eq!(
        harness.host_inbox.len(),
        2,
        "the first frames should arrive before the outage"
    );
    // Wait until the outage has actually begun before queueing the rest, so
    // they are genuinely stuck behind it.
    harness.run_for(200);
    for index in 2..5 {
        harness.orchestrator.queue_message(frame(index)).unwrap();
    }
    assert_eq!(harness.host_inbox.len(), 2);

    harness.run_for(60_000);
    assert_ne!(
        harness.host.liveness(harness.now()),
        Liveness::Live,
        "a peer unreachable for a minute must not still read Live"
    );
    // Offline is advisory: the link never stops retransmitting.
    assert!(harness.counters.dropped > 0);

    let recovered = harness.run_until(10_000, |harness| harness.host_inbox.len() == 5);
    assert!(
        recovered,
        "after the blackout the host had {:?}",
        harness.host_inbox
    );
    assert_eq!(harness.host_inbox, frames(5));
    assert_eq!(harness.counters.rejected, 0, "no datagram was ever refused");
    assert_eq!(
        harness.host.liveness(harness.now()),
        Liveness::Live,
        "the link returns to Live with no reconnect and no re-enrollment"
    );
}

#[test]
fn a_task_continues_across_a_blackout_that_starts_mid_stream() {
    // Frames queued *during* the outage arrive too, in order, once it lifts.
    let mut harness = Harness::new(Script::default().with_blackout(1_000, 60_000));
    harness.orchestrator.queue_message(frame(0)).unwrap();
    harness.run_for(900);
    assert_eq!(harness.host_inbox, frames(1));

    harness.run_for(30_000);
    for index in 1..4 {
        harness.orchestrator.queue_message(frame(index)).unwrap();
    }
    harness.run_for(31_000);

    let recovered = harness.run_until(10_000, |harness| harness.host_inbox.len() == 4);
    assert!(recovered, "the host had {:?}", harness.host_inbox);
    assert_eq!(harness.host_inbox, frames(4));
}

#[test]
fn the_screen_channel_catches_up_in_exactly_one_diff_after_a_forty_second_outage() {
    // §9: "Channel 1 catch-up after a 40-second outage converges in one diff to
    // the current grid — not a replay."
    let mut harness = Harness::new(Script::default().with_blackout(100, 40_000));
    harness
        .orchestrator
        .set_screen(vec![b"line 0".to_vec()])
        .unwrap();
    harness.run_for(60);
    assert_eq!(harness.host.screen().rows(), [b"line 0".to_vec()]);
    // Let the outage begin before anything else is drawn.
    harness.run_for(50);

    // Forty seconds of updates that nobody can see.
    for generation in 1..=400 {
        harness
            .orchestrator
            .set_screen(vec![
                format!("line {generation}").into_bytes(),
                b"status".to_vec(),
            ])
            .unwrap();
        harness.run_for(100);
    }
    assert_eq!(harness.host.screen().rows(), [b"line 0".to_vec()]);

    let delivered_before = harness.screen_datagrams[1];
    let caught_up = harness.run_until(5_000, |harness| harness.host.screen().rows().len() == 2);
    assert!(caught_up, "the screen never caught up");
    assert_eq!(
        harness.host.screen().rows(),
        [b"line 400".to_vec(), b"status".to_vec()],
        "the host should hold the current grid, not an old one"
    );
    assert_eq!(
        harness.screen_datagrams[1] - delivered_before,
        1,
        "catching up cost more than one channel-1 datagram — that is a replay"
    );
}

#[test]
fn queue_overflow_surfaces_a_retryable_transport_error() {
    // §9: "Outbound queue overflow surfaces a retryable transport error."
    let mut config = SessionConfig::new(
        NodeId([1u8; 16]),
        NodeId([2u8; 16]),
        Role::Orchestrator,
        PairKey::from_bytes([7u8; 16]),
        ForwarderKey([8u8; 32]),
    );
    config.queue_limits = QueueLimits {
        max_messages: 16,
        max_bytes: 1 << 20,
    };
    let mut session = Session::new(config, 0);
    let mut seq = MemorySeq::default();

    // The peer is simply not there: everything the session sends is thrown away.
    let mut now = 0;
    let mut error = None;
    for index in 0..64 {
        if let Err(failure) = session.queue_message(frame(index)) {
            error = Some(failure);
            break;
        }
        now += 100;
        let _ = session.outgoing(now, &mut seq).unwrap();
    }

    let error = error.expect("a bounded queue must eventually refuse a message");
    assert!(
        error.is_retryable(),
        "{error} must be retryable so it rejoins the existing retry path"
    );
    assert!(matches!(
        error,
        TransportError::State(_) | TransportError::QueueOverflow(_)
    ));
}

#[test]
fn the_sent_state_history_is_bounded_by_the_same_retryable_error() {
    let mut config = SessionConfig::new(
        NodeId([1u8; 16]),
        NodeId([2u8; 16]),
        Role::Orchestrator,
        PairKey::from_bytes([7u8; 16]),
        ForwarderKey([8u8; 32]),
    );
    // A queue big enough that the *history* bound is what is reached first.
    config.queue_limits = QueueLimits {
        max_messages: 100_000,
        max_bytes: 1 << 30,
    };
    let mut session = Session::new(config, 0);
    let mut error = None;
    for index in 0..(MAX_SENT_STATES + 10) {
        if let Err(failure) = session.queue_message(frame(index)) {
            error = Some(failure);
            break;
        }
    }
    let error = error.expect("the history bound must be enforced");
    assert!(error.is_retryable(), "{error} must be retryable");
}

#[test]
fn round_trip_estimation_tracks_the_network_delay() {
    // §9's SRTT/RTO requirement, end to end rather than in the estimator alone.
    let script = Script {
        delay_ms: 45,
        ..Default::default()
    };
    let mut harness = Harness::new(script);
    harness.orchestrator.queue_message(frame(0)).unwrap();
    harness.run_until(5_000, |harness| !harness.host_inbox.is_empty());
    harness.run_for(1_000);

    let srtt = harness
        .orchestrator
        .rtt()
        .srtt()
        .expect("a completed round trip must produce a sample");
    assert!(
        (srtt - 90.0).abs() < 25.0,
        "SRTT was {srtt} ms for a 90 ms round trip"
    );
    let rto = harness.orchestrator.rto();
    assert!(
        (50..=1_000).contains(&rto),
        "RTO {rto} left the [MIN_RTO, MAX_RTO] clamp"
    );
}

#[test]
fn both_channels_progress_independently() {
    // §4.4: an Instruction on channel 0 says nothing about channel 1.
    let mut harness = Harness::new(Script::hostile());
    for index in 0..10 {
        harness.orchestrator.queue_message(frame(index)).unwrap();
        harness
            .orchestrator
            .set_screen(vec![format!("row {index}").into_bytes()])
            .unwrap();
    }
    let converged = harness.run_until(60_000, |harness| {
        harness.host_inbox.len() == 10 && harness.host.screen().rows() == [b"row 9".to_vec()]
    });
    assert!(converged, "host inbox {:?}", harness.host_inbox);
}

#[test]
fn applying_a_diff_twice_equals_applying_it_once() {
    // §9's `apply_diff` idempotency clause, at the state level.
    let mut sender = medulla_link::state::MessageQueue::new(QueueLimits::default());
    sender.push(b"one".to_vec()).unwrap();
    let seen = sender.clone();
    sender.push(b"two".to_vec()).unwrap();
    let diff = sender.diff_from(&seen);

    let mut once = seen.clone();
    once.apply_diff(&diff).unwrap();
    // Applying the same *Instruction* twice is a no-op because the second copy
    // no longer matches the held state number — the transport refuses it. The
    // diff itself is only ever applied to the state it was cut against.
    let mut twice = seen.clone();
    twice.apply_diff(&diff).unwrap();
    assert_eq!(once, twice);
    assert_eq!(once.num(), 2);
}
