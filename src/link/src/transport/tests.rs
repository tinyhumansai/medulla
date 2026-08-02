//! Unit tests for the per-peer state machine: send scheduling, RTT/RTO,
//! liveness, and the `old_num` rule that makes SSP idempotent.

use super::*;
use crate::keys::{ForwarderKey, MemorySeq, NodeId, PairKey, Role};
use crate::packet::Packet;

/// A pair of sessions that share a pair key, plus their sequence sources.
struct Pair {
    orchestrator: Session,
    host: Session,
    orchestrator_seq: MemorySeq,
    host_seq: MemorySeq,
}

impl Pair {
    fn new() -> Self {
        let pair_key = PairKey::from_bytes([5u8; 16]);
        let orchestrator_id = NodeId([1u8; 16]);
        let host_id = NodeId([2u8; 16]);
        Pair {
            orchestrator: Session::new(
                SessionConfig::new(
                    orchestrator_id,
                    host_id,
                    Role::Orchestrator,
                    pair_key.clone(),
                    ForwarderKey([9u8; 32]),
                ),
                0,
            ),
            host: Session::new(
                SessionConfig::new(
                    host_id,
                    orchestrator_id,
                    Role::Host,
                    pair_key,
                    ForwarderKey([8u8; 32]),
                ),
                0,
            ),
            orchestrator_seq: MemorySeq::default(),
            host_seq: MemorySeq::default(),
        }
    }
}

// ------------------------------------------------------------ idempotency

#[test]
fn applying_the_same_instruction_twice_equals_applying_it_once() {
    // The §9 conformance requirement, and the whole reason duplication is free.
    let mut pair = Pair::new();
    pair.orchestrator.queue_message(b"frame".to_vec()).unwrap();
    let datagrams = pair
        .orchestrator
        .outgoing(100, &mut pair.orchestrator_seq)
        .unwrap();
    assert_eq!(datagrams.len(), 1);

    assert!(pair.host.handle_datagram(&datagrams[0], 110).unwrap());
    let once = pair.host.messages_channel().received_num();
    // The duplicate is refused rather than applied: no second copy of the frame.
    assert!(!pair.host.handle_datagram(&datagrams[0], 120).unwrap());
    assert_eq!(pair.host.messages_channel().received_num(), once);
    assert_eq!(pair.host.take_messages(), vec![b"frame".to_vec()]);
    assert!(pair.host.take_messages().is_empty());
}

#[test]
fn an_instruction_whose_old_num_does_not_match_is_refused() {
    let mut pair = Pair::new();
    pair.orchestrator.queue_message(b"one".to_vec()).unwrap();
    let first = pair
        .orchestrator
        .outgoing(100, &mut pair.orchestrator_seq)
        .unwrap();
    pair.orchestrator.queue_message(b"two".to_vec()).unwrap();
    let second = pair
        .orchestrator
        .outgoing(200, &mut pair.orchestrator_seq)
        .unwrap();

    // The second datagram diffs from state 0 as well (nothing was acked), so
    // deliver it first and then the stale one.
    assert!(pair.host.handle_datagram(&second[0], 210).unwrap());
    assert_eq!(pair.host.messages_channel().received_num(), 2);
    assert!(!pair.host.handle_datagram(&first[0], 220).unwrap());
    assert_eq!(pair.host.messages_channel().received_num(), 2);
    assert_eq!(
        pair.host.take_messages(),
        vec![b"one".to_vec(), b"two".to_vec()]
    );
}

#[test]
fn a_stale_instruction_leaves_the_held_state_byte_for_byte_unchanged() {
    let mut pair = Pair::new();
    pair.orchestrator
        .set_screen(vec![b"row 0".to_vec()])
        .unwrap();
    let first = pair
        .orchestrator
        .outgoing(100, &mut pair.orchestrator_seq)
        .unwrap();
    pair.host.handle_datagram(&first[0], 110).unwrap();
    // Acknowledge, so the next diff starts from state 1 and the replay of the
    // first datagram is genuinely stale.
    let ack = pair.host.outgoing(130, &mut pair.host_seq).unwrap();
    pair.orchestrator.handle_datagram(&ack[0], 140).unwrap();

    pair.orchestrator
        .set_screen(vec![b"row 0".to_vec(), b"row 1".to_vec()])
        .unwrap();
    let second = pair
        .orchestrator
        .outgoing(200, &mut pair.orchestrator_seq)
        .unwrap();
    pair.host.handle_datagram(&second[0], 210).unwrap();
    let after = pair.host.screen().clone();

    assert!(!pair.host.handle_datagram(&first[0], 220).unwrap());
    assert_eq!(pair.host.screen(), &after);
}

// -------------------------------------------------------------- addressing

#[test]
fn a_datagram_for_another_node_is_refused() {
    let mut pair = Pair::new();
    pair.orchestrator.queue_message(b"frame".to_vec()).unwrap();
    let datagrams = pair
        .orchestrator
        .outgoing(100, &mut pair.orchestrator_seq)
        .unwrap();
    // The host's own session must not accept its own datagram back.
    assert!(matches!(
        pair.orchestrator.handle_datagram(&datagrams[0], 110),
        Err(TransportError::Misaddressed(_))
    ));
}

#[test]
fn a_tampered_payload_is_refused() {
    let mut pair = Pair::new();
    pair.orchestrator.queue_message(b"frame".to_vec()).unwrap();
    let mut datagram = pair
        .orchestrator
        .outgoing(100, &mut pair.orchestrator_seq)
        .unwrap()
        .remove(0);
    let last = datagram.len() - 1;
    datagram[last] ^= 0x01;
    assert!(matches!(
        pair.host.handle_datagram(&datagram, 110),
        Err(TransportError::Crypto(_))
    ));
}

#[test]
fn an_unknown_channel_is_ignored_rather_than_failing_the_datagram() {
    // Forward compatibility: a later dialect must not break this endpoint.
    let mut pair = Pair::new();
    pair.orchestrator.queue_message(b"frame".to_vec()).unwrap();
    let datagram = pair
        .orchestrator
        .outgoing(100, &mut pair.orchestrator_seq)
        .unwrap()
        .remove(0);
    // Re-seal the same datagram with the channel byte changed to 7.
    let header = crate::header::OuterHeader::decode(&datagram).unwrap();
    let plaintext = crate::crypto::open(
        &PairKey::from_bytes([5u8; 16]),
        header.seq,
        &header.signed_bytes(),
        &datagram[crate::header::HEADER_LEN..],
    )
    .unwrap();
    let mut packet = Packet::decode(&plaintext).unwrap();
    packet.instruction.channel = 7;
    let mut rebuilt = datagram[..crate::header::HEADER_LEN].to_vec();
    rebuilt.extend_from_slice(&crate::crypto::seal(
        &PairKey::from_bytes([5u8; 16]),
        header.seq,
        &header.signed_bytes(),
        &packet.encode().unwrap(),
    ));
    assert!(!pair.host.handle_datagram(&rebuilt, 110).unwrap());
}

// ------------------------------------------------------------------ timing

#[test]
fn rto_is_srtt_plus_four_rttvar_clamped_to_the_protocol_constants() {
    let mut rtt = RttEstimator::new();
    // With no sample the estimator is deliberately conservative.
    assert_eq!(rtt.rto(), MAX_RTO);

    // First sample: SRTT = r, RTTVAR = r/2, so RTO = 3r — clamped at 1000 ms.
    rtt.sample(100);
    assert_eq!(rtt.srtt(), Some(100.0));
    assert_eq!(rtt.rttvar(), 50.0);
    assert_eq!(rtt.rto(), 300);

    // A steady RTT decays RTTVAR toward zero, so RTO converges on SRTT.
    for _ in 0..200 {
        rtt.sample(100);
    }
    assert!((rtt.srtt().unwrap() - 100.0).abs() < 0.5);
    assert!(rtt.rttvar() < 0.5);
    assert_eq!(rtt.rto(), MIN_RTO.max(100));
}

#[test]
fn rto_never_leaves_its_clamp() {
    let mut fast = RttEstimator::new();
    fast.sample(0);
    assert_eq!(fast.rto(), MIN_RTO);

    let mut slow = RttEstimator::new();
    slow.sample(5_000);
    assert_eq!(slow.rto(), MAX_RTO);
}

#[test]
fn a_round_trip_produces_a_sample() {
    let mut pair = Pair::new();
    pair.orchestrator.queue_message(b"frame".to_vec()).unwrap();
    let out = pair
        .orchestrator
        .outgoing(1_000, &mut pair.orchestrator_seq)
        .unwrap();
    assert!(pair.orchestrator.rtt().srtt().is_none());

    pair.host.handle_datagram(&out[0], 1_040).unwrap();
    let back = pair.host.outgoing(1_060, &mut pair.host_seq).unwrap();
    pair.orchestrator.handle_datagram(&back[0], 1_080).unwrap();

    // 80 ms of round trip, minus the 20 ms the host sat on the timestamp.
    assert_eq!(pair.orchestrator.rtt().srtt(), Some(60.0));
}

#[test]
fn liveness_follows_the_documented_windows() {
    assert_eq!(Liveness::from_silence(0), Liveness::Live);
    assert_eq!(Liveness::from_silence(LIVE_WINDOW), Liveness::Live);
    assert_eq!(Liveness::from_silence(LIVE_WINDOW + 1), Liveness::Degraded);
    assert_eq!(Liveness::from_silence(DEGRADED_WINDOW), Liveness::Degraded);
    assert_eq!(
        Liveness::from_silence(DEGRADED_WINDOW + 1),
        Liveness::Offline
    );
}

#[test]
fn a_session_that_has_never_heard_from_its_peer_ages_from_its_start() {
    let pair = Pair::new();
    assert_eq!(pair.host.liveness(0), Liveness::Live);
    assert_eq!(pair.host.liveness(10_000), Liveness::Degraded);
    assert_eq!(pair.host.liveness(120_000), Liveness::Offline);
}

#[test]
fn an_idle_session_sends_one_heartbeat_every_three_seconds() {
    let mut pair = Pair::new();
    // The very first call has nothing to acknowledge and nothing to send, so it
    // heartbeats immediately and then holds off.
    assert_eq!(pair.host.outgoing(0, &mut pair.host_seq).unwrap().len(), 1);
    assert!(pair
        .host
        .outgoing(HEARTBEAT - 1, &mut pair.host_seq)
        .unwrap()
        .is_empty());
    assert_eq!(
        pair.host
            .outgoing(HEARTBEAT, &mut pair.host_seq)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(pair.host.next_send_ms(HEARTBEAT), 2 * HEARTBEAT);
}

#[test]
fn a_heartbeat_is_flagged_and_a_state_change_is_not() {
    let mut pair = Pair::new();
    let beat = pair.host.outgoing(0, &mut pair.host_seq).unwrap().remove(0);
    assert!(crate::header::OuterHeader::decode(&beat)
        .unwrap()
        .is_heartbeat());

    pair.host.queue_message(b"frame".to_vec()).unwrap();
    let carried = pair
        .host
        .outgoing(SEND_INTERVAL_MIN, &mut pair.host_seq)
        .unwrap()
        .remove(0);
    assert!(!crate::header::OuterHeader::decode(&carried)
        .unwrap()
        .is_heartbeat());
}

#[test]
fn a_new_state_change_waits_only_for_the_send_interval_floor() {
    let mut pair = Pair::new();
    pair.host.outgoing(0, &mut pair.host_seq).unwrap();
    pair.host.queue_message(b"frame".to_vec()).unwrap();
    assert!(pair
        .host
        .outgoing(SEND_INTERVAL_MIN - 1, &mut pair.host_seq)
        .unwrap()
        .is_empty());
    assert_eq!(
        pair.host
            .outgoing(SEND_INTERVAL_MIN, &mut pair.host_seq)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn an_unacknowledged_state_is_retransmitted_after_the_rto() {
    let mut pair = Pair::new();
    pair.host.queue_message(b"frame".to_vec()).unwrap();
    let first = pair.host.outgoing(0, &mut pair.host_seq).unwrap();
    assert_eq!(first.len(), 1);
    let rto = pair.host.rto();
    assert!(pair
        .host
        .outgoing(rto - 1, &mut pair.host_seq)
        .unwrap()
        .is_empty());
    assert_eq!(
        pair.host.outgoing(rto, &mut pair.host_seq).unwrap().len(),
        1
    );
    assert!(pair.host.has_pending_state());
}

#[test]
fn an_acknowledgement_stops_the_retransmission() {
    let mut pair = Pair::new();
    pair.orchestrator.queue_message(b"frame".to_vec()).unwrap();
    let out = pair
        .orchestrator
        .outgoing(0, &mut pair.orchestrator_seq)
        .unwrap();
    pair.host.handle_datagram(&out[0], 10).unwrap();
    let ack = pair.host.outgoing(30, &mut pair.host_seq).unwrap();
    assert_eq!(ack.len(), 1, "the host owes an acknowledgement");
    pair.orchestrator.handle_datagram(&ack[0], 40).unwrap();

    assert!(!pair.orchestrator.has_pending_state());
    assert!(pair
        .orchestrator
        .outgoing(1_000, &mut pair.orchestrator_seq)
        .unwrap()
        .is_empty());
}

#[test]
fn each_channel_is_acknowledged_on_its_own() {
    // §4.4: an Instruction on channel 0 says nothing about channel 1.
    let mut pair = Pair::new();
    pair.orchestrator.queue_message(b"frame".to_vec()).unwrap();
    pair.orchestrator.set_screen(vec![b"row".to_vec()]).unwrap();
    let out = pair
        .orchestrator
        .outgoing(0, &mut pair.orchestrator_seq)
        .unwrap();
    assert_eq!(out.len(), 2, "both channels had work");

    for datagram in &out {
        pair.host.handle_datagram(datagram, 10).unwrap();
    }
    // Two acknowledgements, one per channel, then nothing more.
    let first = pair.host.outgoing(30, &mut pair.host_seq).unwrap();
    let second = pair.host.outgoing(60, &mut pair.host_seq).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    for datagram in first.iter().chain(second.iter()) {
        pair.orchestrator.handle_datagram(datagram, 80).unwrap();
    }
    assert!(!pair.orchestrator.has_pending_state());
    assert_eq!(pair.host.screen().rows(), [b"row".to_vec()]);
}

// ------------------------------------------------------------------ bounds

#[test]
fn a_message_larger_than_a_datagram_is_refused_at_enqueue() {
    let mut pair = Pair::new();
    let error = pair
        .host
        .queue_message(vec![0u8; MAX_MESSAGE_BYTES + 1])
        .unwrap_err();
    assert!(matches!(error, TransportError::DiffTooLarge { .. }));
    assert!(!error.is_retryable());
    // The largest permitted message still produces a legal datagram.
    pair.host
        .queue_message(vec![0u8; MAX_MESSAGE_BYTES])
        .unwrap();
    let datagram = pair.host.outgoing(0, &mut pair.host_seq).unwrap().remove(0);
    assert_eq!(datagram.len(), crate::header::MAX_DATAGRAM);
}

#[test]
fn queue_overflow_is_reported_as_retryable() {
    let mut config = SessionConfig::new(
        NodeId([1u8; 16]),
        NodeId([2u8; 16]),
        Role::Host,
        PairKey::from_bytes([5u8; 16]),
        ForwarderKey([9u8; 32]),
    );
    config.queue_limits.max_messages = 2;
    let mut session = Session::new(config, 0);
    session.queue_message(b"a".to_vec()).unwrap();
    session.queue_message(b"b".to_vec()).unwrap();
    let error = session.queue_message(b"c".to_vec()).unwrap_err();
    assert!(error.is_retryable(), "{error} should be retryable");
}

#[test]
fn history_is_freed_as_the_peer_acknowledges() {
    let mut pair = Pair::new();
    for index in 0..8u8 {
        pair.orchestrator.queue_message(vec![index]).unwrap();
    }
    assert_eq!(pair.orchestrator.messages_channel().history_len(), 9);

    let mut now = 0;
    for _ in 0..8 {
        for datagram in pair
            .orchestrator
            .outgoing(now, &mut pair.orchestrator_seq)
            .unwrap()
        {
            pair.host.handle_datagram(&datagram, now + 5).unwrap();
        }
        now += 100;
        for datagram in pair.host.outgoing(now, &mut pair.host_seq).unwrap() {
            pair.orchestrator
                .handle_datagram(&datagram, now + 5)
                .unwrap();
        }
        now += 100;
    }
    assert_eq!(
        pair.orchestrator.messages_channel().history_len(),
        1,
        "only the assumed receiver state should remain"
    );
    assert_eq!(
        pair.orchestrator.messages_channel().assumed_receiver_num(),
        8
    );
    assert_eq!(pair.host.take_messages().len(), 8);
}

#[test]
fn the_status_snapshot_reports_liveness_and_timing() {
    let mut pair = Pair::new();
    let status = pair.host.status(0);
    assert_eq!(status.liveness, Liveness::Live);
    assert_eq!(status.srtt_ms, None);
    assert_eq!(status.rto_ms, MAX_RTO);
    assert!(!status.pending);

    pair.host.queue_message(b"frame".to_vec()).unwrap();
    assert!(pair.host.status(0).pending);
    assert_eq!(pair.host.status(120_000).liveness, Liveness::Offline);
}
