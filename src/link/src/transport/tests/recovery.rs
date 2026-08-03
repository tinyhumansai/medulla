//! Recovery tests for bidirectional delivery and endpoint restarts.

use super::*;

#[test]
fn a_bidirectional_task_round_trip_delivers_the_terminal_frame() {
    let mut pair = Pair::new();
    pair.orchestrator.queue_message(b"task".to_vec()).unwrap();
    let task = pair
        .orchestrator
        .outgoing(100, &mut pair.orchestrator_seq)
        .unwrap();
    pair.host.handle_datagram(&task[0], 110).unwrap();
    assert_eq!(pair.host.take_messages(), vec![b"task".to_vec()]);

    pair.host.queue_message(b"status".to_vec()).unwrap();
    let status = pair.host.outgoing(120, &mut pair.host_seq).unwrap();
    pair.orchestrator.handle_datagram(&status[0], 130).unwrap();
    assert_eq!(pair.orchestrator.take_messages(), vec![b"status".to_vec()]);

    pair.host.queue_message(b"terminal".to_vec()).unwrap();
    let terminal = pair.host.outgoing(140, &mut pair.host_seq).unwrap();
    pair.orchestrator
        .handle_datagram(&terminal[0], 150)
        .unwrap();
    let ack = pair
        .orchestrator
        .outgoing(160, &mut pair.orchestrator_seq)
        .unwrap();
    pair.host.handle_datagram(&ack[0], 170).unwrap();
    let retry = pair.host.outgoing(1_000, &mut pair.host_seq).unwrap();
    pair.orchestrator.handle_datagram(&retry[0], 190).unwrap();
    assert_eq!(
        pair.orchestrator.take_messages(),
        vec![b"terminal".to_vec()]
    );
}

#[test]
fn one_endpoint_restart_rebases_the_live_peer_and_delivers_new_work() {
    let mut pair = Pair::new();
    pair.orchestrator.queue_message(b"before".to_vec()).unwrap();
    let first = pair
        .orchestrator
        .outgoing(100, &mut pair.orchestrator_seq)
        .unwrap();
    pair.host.handle_datagram(&first[0], 110).unwrap();
    assert_eq!(pair.host.take_messages(), vec![b"before".to_vec()]);
    let ack = pair.host.outgoing(140, &mut pair.host_seq).unwrap();
    pair.orchestrator.handle_datagram(&ack[0], 150).unwrap();

    pair.host = Session::new(
        SessionConfig::new(
            NodeId([2u8; 16]),
            NodeId([1u8; 16]),
            Role::Host,
            PairKey::from_bytes([5u8; 16]),
            ForwarderKey([8u8; 32]),
        ),
        200,
    );
    let hello = pair.host.outgoing(200, &mut pair.host_seq).unwrap();
    pair.orchestrator.handle_datagram(&hello[0], 210).unwrap();
    assert!(!pair.orchestrator.handle_datagram(&ack[0], 215).unwrap());

    pair.orchestrator.queue_message(b"after".to_vec()).unwrap();
    let after = pair
        .orchestrator
        .outgoing(220, &mut pair.orchestrator_seq)
        .unwrap();
    for datagram in after {
        pair.host.handle_datagram(&datagram, 230).unwrap();
    }
    assert_eq!(pair.host.take_messages(), vec![b"after".to_vec()]);
}

#[test]
fn a_peer_restart_preserves_every_pending_message_on_channel_zero() {
    let mut pair = Pair::new();
    pair.orchestrator.queue_message(b"status".to_vec()).unwrap();
    pair.orchestrator
        .queue_message(b"terminal".to_vec())
        .unwrap();

    pair.host = Session::new(
        SessionConfig::new(
            NodeId([2u8; 16]),
            NodeId([1u8; 16]),
            Role::Host,
            PairKey::from_bytes([5u8; 16]),
            ForwarderKey([8u8; 32]),
        ),
        200,
    );
    let hello = pair.host.outgoing(200, &mut pair.host_seq).unwrap();
    pair.orchestrator.handle_datagram(&hello[0], 210).unwrap();

    let pending = pair
        .orchestrator
        .outgoing(220, &mut pair.orchestrator_seq)
        .unwrap();
    for datagram in pending {
        pair.host.handle_datagram(&datagram, 230).unwrap();
    }
    assert_eq!(
        pair.host.take_messages(),
        vec![b"status".to_vec(), b"terminal".to_vec()]
    );
}

#[test]
fn a_peer_restart_preserves_datagram_sized_queue_prefixes() {
    let mut pair = Pair::new();
    let first = vec![1; MAX_MESSAGE_BYTES];
    let second = vec![2; MAX_MESSAGE_BYTES];
    pair.orchestrator.queue_message(first.clone()).unwrap();
    pair.orchestrator.queue_message(second.clone()).unwrap();

    pair.host = Session::new(
        SessionConfig::new(
            NodeId([2u8; 16]),
            NodeId([1u8; 16]),
            Role::Host,
            PairKey::from_bytes([5u8; 16]),
            ForwarderKey([8u8; 32]),
        ),
        200,
    );
    let hello = pair.host.outgoing(200, &mut pair.host_seq).unwrap();
    pair.orchestrator.handle_datagram(&hello[0], 210).unwrap();

    let first_step = pair
        .orchestrator
        .outgoing(220, &mut pair.orchestrator_seq)
        .unwrap();
    pair.host.handle_datagram(&first_step[0], 230).unwrap();
    let ack = pair.host.outgoing(240, &mut pair.host_seq).unwrap();
    pair.orchestrator.handle_datagram(&ack[0], 250).unwrap();
    let second_step = pair
        .orchestrator
        .outgoing(260, &mut pair.orchestrator_seq)
        .unwrap();
    pair.host.handle_datagram(&second_step[0], 270).unwrap();

    assert_eq!(pair.host.take_messages(), vec![first, second]);
}

#[test]
fn a_peer_restart_rebuilds_screen_rows_as_datagram_sized_prefixes() {
    let mut pair = Pair::new();
    let rows = vec![vec![1; 900], vec![2; 900], vec![3; 900]];
    for end in 1..=rows.len() {
        pair.orchestrator.set_screen(rows[..end].to_vec()).unwrap();
    }

    let first = pair
        .orchestrator
        .outgoing(100, &mut pair.orchestrator_seq)
        .unwrap();
    pair.host.handle_datagram(&first[0], 110).unwrap();
    let ack = pair.host.outgoing(120, &mut pair.host_seq).unwrap();
    pair.orchestrator.handle_datagram(&ack[0], 130).unwrap();

    pair.host = Session::new(
        SessionConfig::new(
            NodeId([2u8; 16]),
            NodeId([1u8; 16]),
            Role::Host,
            PairKey::from_bytes([5u8; 16]),
            ForwarderKey([8u8; 32]),
        ),
        200,
    );
    let hello = pair.host.outgoing(200, &mut pair.host_seq).unwrap();
    pair.orchestrator.handle_datagram(&hello[0], 210).unwrap();

    for step in 0..3 {
        let base = 300 + step * 100;
        let sent = pair
            .orchestrator
            .outgoing(base, &mut pair.orchestrator_seq)
            .unwrap();
        pair.host.handle_datagram(&sent[0], base + 10).unwrap();
        let ack = pair.host.outgoing(base + 20, &mut pair.host_seq).unwrap();
        pair.orchestrator
            .handle_datagram(&ack[0], base + 30)
            .unwrap();
    }

    assert_eq!(pair.host.screen().rows(), rows);
}
