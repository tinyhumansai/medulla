//! Unit tests for direct-mode key derivation, routing policy and address
//! learning.
//!
//! The roaming rule is the part worth testing hardest, because getting it wrong
//! is not a crash — it is a link that a captured datagram can quietly steer into
//! a blackhole. Each half of the rule ("authentic" and "fresh") gets a test that
//! fails if only the other half is enforced.

use super::*;
use crate::keys::{MemorySeq, NodeId, PairKey, Role};
use crate::state::QueueLimits;
use crate::transport::SessionConfig;
use crate::transport::MAX_SENT_STATES;

/// A loopback address that differs from [`elsewhere`], for the roaming tests.
fn somewhere() -> SocketAddr {
    "127.0.0.1:9001".parse().expect("a literal address parses")
}

/// A second address, standing in for "the peer moved" or "an attacker replayed".
fn elsewhere() -> SocketAddr {
    "127.0.0.1:9002".parse().expect("a literal address parses")
}

/// Two sessions keyed to each other, direct-mode, ready to exchange datagrams.
fn pair() -> (Session, Session, PairKey) {
    let orchestrator = NodeId::generate();
    let host = NodeId::generate();
    let pair_key = PairKey::generate();
    let outer = path_key(&pair_key);
    let config = |node_id, peer_node_id, role| SessionConfig {
        node_id,
        peer_node_id,
        role,
        pair_key: pair_key.clone(),
        forwarder_key: outer.clone(),
        queue_limits: QueueLimits::default(),
        max_sent_states: MAX_SENT_STATES,
    };
    (
        Session::new(config(orchestrator, host, Role::Orchestrator), 0),
        Session::new(config(host, orchestrator, Role::Host), 0),
        pair_key,
    )
}

#[test]
fn the_path_key_is_derived_from_the_pair_key_and_nothing_else() {
    let key = PairKey::generate();
    assert_eq!(
        path_key(&key).as_bytes(),
        path_key(&key).as_bytes(),
        "derivation must be deterministic, or the two ends disagree"
    );
    assert_ne!(
        path_key(&key).as_bytes(),
        path_key(&PairKey::generate()).as_bytes(),
        "a different pair key must give a different path key"
    );
}

#[test]
fn the_path_key_is_domain_separated_from_the_aead_key() {
    // Both are SHA-256 over the same pair key; only the label separates them.
    // If the labels ever collide, one key would authenticate what the other
    // encrypts, which is exactly what domain separation exists to prevent.
    let key = PairKey::generate();
    let mut sealed = Vec::new();
    sealed.extend_from_slice(b"medulla-link/1 pair-key");
    sealed.extend_from_slice(key.as_bytes());
    let aead: [u8; 32] = {
        use sha2::{Digest, Sha256};
        Sha256::digest(&sealed).into()
    };
    assert_ne!(&aead, path_key(&key).as_bytes());
}

#[test]
fn a_forwarded_route_accepts_only_the_forwarder() {
    let route = Route::Forwarder(somewhere());
    assert!(route.accepts(somewhere()));
    assert!(!route.accepts(elsewhere()));
}

#[test]
fn a_direct_route_accepts_any_source_and_lets_the_aead_decide() {
    // Not laxness: a datagram from an address we have never seen is precisely
    // what a peer that has roamed looks like, so the address cannot be the
    // filter. Authentication is.
    let route = Route::Direct(somewhere());
    assert!(route.accepts(somewhere()));
    assert!(route.accepts(elsewhere()));
}

#[test]
fn learning_never_moves_a_forwarded_route() {
    let (mut orchestrator, mut host, _) = pair();
    let mut seq = MemorySeq::default();
    orchestrator.queue_message(b"hello".to_vec()).unwrap();
    let datagram = orchestrator.outgoing(0, &mut seq).unwrap().remove(0);
    host.handle_datagram(&datagram, 0).unwrap();

    let mut route = Route::Forwarder(somewhere());
    learn(&mut route, &host, elsewhere());
    assert!(
        matches!(route, Route::Forwarder(address) if address == somewhere()),
        "the forwarder relearns addresses itself; this link must not second-guess it"
    );
}

#[test]
fn a_fresh_datagram_moves_a_direct_route() {
    let (mut orchestrator, mut host, _) = pair();
    let mut seq = MemorySeq::default();
    orchestrator.queue_message(b"hello".to_vec()).unwrap();
    let datagram = orchestrator.outgoing(0, &mut seq).unwrap().remove(0);
    host.handle_datagram(&datagram, 0).unwrap();

    let mut route = Route::Direct(somewhere());
    learn(&mut route, &host, elsewhere());
    assert!(
        matches!(route, Route::Direct(address) if address == elsewhere()),
        "a peer that moved networks must be followed, or the session is lost"
    );
}

#[test]
fn a_replayed_datagram_does_not_move_a_direct_route() {
    // The attack this rule exists to stop. Every datagram stays authentic
    // forever, so an off-path attacker who captured one and resent it from
    // their own address would steal the flow into a blackhole — they hold no
    // key, so nothing they send is accepted, and the victim never hears the
    // acknowledgement that would correct it.
    let (mut orchestrator, mut host, _) = pair();
    let mut seq = MemorySeq::default();
    orchestrator.queue_message(b"first".to_vec()).unwrap();
    let first = orchestrator.outgoing(0, &mut seq).unwrap().remove(0);
    orchestrator.queue_message(b"second".to_vec()).unwrap();
    let second = orchestrator.outgoing(1_000, &mut seq).unwrap().remove(0);

    host.handle_datagram(&first, 0).unwrap();
    host.handle_datagram(&second, 1_000).unwrap();

    let mut route = Route::Direct(somewhere());
    // The attacker resends the *older* datagram from their own address.
    host.handle_datagram(&first, 2_000).unwrap();
    learn(&mut route, &host, elsewhere());
    assert!(
        matches!(route, Route::Direct(address) if address == somewhere()),
        "a replay must not be able to steer the link"
    );
}

#[test]
fn learning_is_a_no_op_when_the_peer_has_not_moved() {
    let (mut orchestrator, mut host, _) = pair();
    let mut seq = MemorySeq::default();
    orchestrator.queue_message(b"hello".to_vec()).unwrap();
    let datagram = orchestrator.outgoing(0, &mut seq).unwrap().remove(0);
    host.handle_datagram(&datagram, 0).unwrap();

    let mut route = Route::Direct(somewhere());
    learn(&mut route, &host, somewhere());
    assert!(matches!(route, Route::Direct(address) if address == somewhere()));
}
