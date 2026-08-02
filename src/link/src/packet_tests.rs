//! Tests for the plaintext framing and the `Instruction` (protocol §4.1, §4.3).

use super::*;

fn instruction() -> Instruction {
    Instruction {
        channel: 1,
        old_num: 7,
        new_num: 9,
        ack_num: 4,
        throwaway_num: 3,
        diff: b"rows".to_vec(),
    }
}

#[test]
fn the_instruction_header_is_thirty_seven_bytes() {
    let mut encoded = Vec::new();
    instruction().encode_into(&mut encoded).unwrap();
    assert_eq!(INSTRUCTION_HEADER_LEN, 37);
    assert_eq!(encoded.len(), INSTRUCTION_HEADER_LEN + 4);
}

#[test]
fn an_instruction_round_trips() {
    let mut encoded = Vec::new();
    instruction().encode_into(&mut encoded).unwrap();
    assert_eq!(Instruction::decode(&encoded).unwrap(), instruction());
}

#[test]
fn a_packet_round_trips_with_its_timestamps() {
    let packet = Packet {
        send_ts: 1234,
        reply_ts: 0,
        instruction: instruction(),
    };
    let encoded = packet.encode().unwrap();
    assert_eq!(Packet::decode(&encoded).unwrap(), packet);
}

#[test]
fn a_truncated_instruction_is_rejected() {
    let mut encoded = Vec::new();
    instruction().encode_into(&mut encoded).unwrap();
    let short = &encoded[..20];
    assert!(matches!(
        Instruction::decode(short),
        Err(PacketError::Truncated { .. })
    ));
}

#[test]
fn a_lying_diff_len_is_rejected() {
    let mut encoded = Vec::new();
    instruction().encode_into(&mut encoded).unwrap();
    encoded[36] = 9;
    assert_eq!(
        Instruction::decode(&encoded),
        Err(PacketError::DiffLength {
            declared: 9,
            actual: 4
        })
    );
}

#[test]
fn a_packet_without_timestamps_is_rejected() {
    assert_eq!(
        Packet::decode(&[0u8, 1]),
        Err(PacketError::Truncated {
            expected: TIMESTAMP_LEN,
            actual: 2
        })
    );
}

#[test]
fn a_diff_beyond_the_datagram_budget_is_refused() {
    let oversized = Instruction {
        diff: vec![0u8; MAX_DIFF_LEN + 1],
        ..instruction()
    };
    assert_eq!(
        oversized.encode_into(&mut Vec::new()),
        Err(PacketError::DiffTooLarge(MAX_DIFF_LEN + 1))
    );
}

#[test]
fn a_maximum_sized_packet_fits_one_datagram() {
    // §3.2's arithmetic, asserted rather than trusted.
    let packet = Packet {
        send_ts: 1,
        reply_ts: 2,
        instruction: Instruction {
            diff: vec![0u8; MAX_DIFF_LEN],
            ..instruction()
        },
    };
    let plaintext = packet.encode().unwrap();
    assert_eq!(plaintext.len(), MAX_PLAINTEXT);
    assert_eq!(
        crate::header::HEADER_LEN + plaintext.len() + crate::crypto::AEAD_OVERHEAD,
        crate::header::MAX_DATAGRAM
    );
}

#[test]
fn timestamps_are_milliseconds_modulo_two_to_the_sixteen() {
    assert_eq!(timestamp16(0), 0);
    assert_eq!(timestamp16(65_535), 65_535);
    assert_eq!(timestamp16(65_536), 0);
    assert_eq!(timestamp16(65_540), 4);
}

#[test]
fn elapsed_unwraps_across_the_sixteen_bit_boundary() {
    assert_eq!(elapsed16(1_000, 900), 100);
    // The peer's timestamp was taken just before the field wrapped.
    assert_eq!(elapsed16(65_540, 65_530), 10);
}
