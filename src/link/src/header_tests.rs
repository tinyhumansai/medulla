//! Tests for the cleartext outer header (protocol §3).

use super::*;
use crate::keys::{ForwarderKey, NodeId};

fn key() -> ForwarderKey {
    ForwarderKey([3u8; 32])
}

fn header() -> OuterHeader {
    OuterHeader::new(NodeId([1u8; 16]), NodeId([2u8; 16]), 0x8000_0000_0000_002A)
}

#[test]
fn the_header_is_fifty_eight_bytes_with_the_tag_at_forty_two() {
    let encoded = header().encode(&key());
    assert_eq!(encoded.len(), 58);
    assert_eq!(HEADER_LEN, 58);
    assert_eq!(TAG_OFFSET, 42);
    assert_eq!(encoded[0], VERSION);
}

#[test]
fn fields_sit_at_their_documented_offsets() {
    let encoded = header().encode(&key());
    assert_eq!(&encoded[2..18], &[1u8; 16]);
    assert_eq!(&encoded[18..34], &[2u8; 16]);
    assert_eq!(
        u64::from_be_bytes(encoded[34..42].try_into().unwrap()),
        0x8000_0000_0000_002A
    );
}

#[test]
fn encode_then_decode_round_trips() {
    let encoded = header().encode(&key());
    assert_eq!(OuterHeader::decode(&encoded).unwrap(), header());
}

#[test]
fn a_valid_tag_verifies_and_a_flipped_bit_does_not() {
    let mut encoded = header().encode(&key());
    assert!(verify_tag(&encoded, &key()));
    encoded[TAG_OFFSET] ^= 0x01;
    assert!(!verify_tag(&encoded, &key()));
}

#[test]
fn a_rewritten_field_invalidates_the_tag() {
    // The forwarder must not rewrite anything (§5 rule 8); this is why.
    let mut encoded = header().encode(&key());
    encoded[34] ^= 0x01;
    assert!(!verify_tag(&encoded, &key()));
}

#[test]
fn another_nodes_key_does_not_verify() {
    let encoded = header().encode(&key());
    assert!(!verify_tag(&encoded, &ForwarderKey([4u8; 32])));
}

#[test]
fn a_short_datagram_is_rejected() {
    assert_eq!(
        OuterHeader::decode(&[0u8; 10]),
        Err(HeaderError::TooShort(10))
    );
    assert!(!verify_tag(&[0u8; 10], &key()));
}

#[test]
fn an_unknown_version_is_rejected() {
    let mut encoded = header().encode(&key()).to_vec();
    encoded[0] = 2;
    assert_eq!(OuterHeader::decode(&encoded), Err(HeaderError::Version(2)));
}

#[test]
fn a_reserved_flag_bit_is_rejected() {
    let mut encoded = header().encode(&key()).to_vec();
    encoded[1] = 0b0000_0010;
    assert_eq!(
        OuterHeader::decode(&encoded),
        Err(HeaderError::ReservedFlags(0b0000_0010))
    );
}

#[test]
fn the_heartbeat_flag_survives_a_round_trip() {
    let beat = header().heartbeat();
    assert!(beat.is_heartbeat());
    let decoded = OuterHeader::decode(&beat.encode(&key())).unwrap();
    assert!(decoded.is_heartbeat());
    assert!(!header().is_heartbeat());
}

#[test]
fn the_signed_bytes_are_the_header_without_its_tag() {
    let encoded = header().encode(&key());
    assert_eq!(&header().signed_bytes()[..], &encoded[..TAG_OFFSET]);
}
