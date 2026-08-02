//! Tests for the payload AEAD (protocol §4).

use super::*;
use crate::keys::{NodeId, Role};

fn key() -> PairKey {
    PairKey::from_bytes([7u8; 16])
}

fn aad() -> Vec<u8> {
    crate::header::OuterHeader::new(NodeId([1u8; 16]), NodeId([2u8; 16]), 9)
        .signed_bytes()
        .to_vec()
}

#[test]
fn seal_then_open_round_trips() {
    let sealed = seal(&key(), 9, &aad(), b"hello link");
    let opened = open(&key(), 9, &aad(), &sealed).expect("same key, seq and aad");
    assert_eq!(opened, b"hello link");
}

#[test]
fn a_different_sequence_fails_to_open() {
    let sealed = seal(&key(), 9, &aad(), b"hello link");
    assert_eq!(
        open(&key(), 10, &aad(), &sealed),
        Err(CryptoError::Authentication)
    );
}

#[test]
fn a_rewritten_header_fails_to_open() {
    // §5 rule 8: a forwarder that rewrote any header field would break this.
    let sealed = seal(&key(), 9, &aad(), b"hello link");
    let mut tampered = aad();
    tampered[2] ^= 0x01;
    assert_eq!(
        open(&key(), 9, &tampered, &sealed),
        Err(CryptoError::Authentication)
    );
}

#[test]
fn a_different_pair_key_fails_to_open() {
    let sealed = seal(&key(), 9, &aad(), b"hello link");
    let other = PairKey::from_bytes([8u8; 16]);
    assert_eq!(
        open(&other, 9, &aad(), &sealed),
        Err(CryptoError::Authentication)
    );
}

#[test]
fn nonce_is_four_zero_bytes_then_big_endian_seq() {
    assert_eq!(nonce_for(1), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    assert_eq!(
        nonce_for(DIRECTION_MASK | 1),
        [0, 0, 0, 0, 0x80, 0, 0, 0, 0, 0, 0, 1]
    );
}

#[test]
fn the_direction_bit_keeps_the_two_endpoints_nonce_spaces_disjoint() {
    // The whole reason one pair key is safe in both directions (§4.2).
    let orchestrator = Role::Orchestrator.direction_bit() | 42;
    let host = Role::Host.direction_bit() | 42;
    assert_ne!(nonce_for(orchestrator), nonce_for(host));
    assert!(orchestrator >= DIRECTION_MASK);
    assert!(host < DIRECTION_MASK);
}

#[test]
fn the_aead_adds_exactly_the_declared_overhead() {
    let sealed = seal(&key(), 1, &aad(), b"1234567890");
    assert_eq!(sealed.len(), 10 + AEAD_OVERHEAD);
}
