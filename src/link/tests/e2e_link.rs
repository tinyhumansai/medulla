//! Two `Link`s over real UDP sockets, through a minimal stand-in forwarder.
//!
//! The conformance suite drives the state machine directly, which is where the
//! interesting cases live. This suite covers the wiring the state machine cannot
//! see: `Link::connect` opening an identity, the driver task, the inbound queue
//! and `status()`. The forwarder here implements only what §5 needs for two
//! nodes to reach each other — the real one, its replay window and its team
//! rules belong to the backend.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use medulla_link::header::{verify_tag, OuterHeader};
use medulla_link::keys::{self, ForwarderKey, NodeId, NodeState, PairKey, Role};
use medulla_link::{Link, LinkConfig, Liveness};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;

/// How long to wait for a datagram to make a loopback round trip.
const PATIENCE: Duration = Duration::from_secs(5);

/// Write an identity into `dir` so `Link::connect` can open it.
fn enroll(
    dir: &Path,
    node_id: NodeId,
    peer_node_id: NodeId,
    role: Role,
    pair_key: PairKey,
    forwarder_key: ForwarderKey,
    forwarder_endpoint: String,
) {
    keys::acquire_or_create(dir, || NodeState {
        version: 1,
        node_id,
        role,
        pair_key,
        forwarder_key,
        forwarder_endpoint,
        peer_node_id,
        peers: Vec::new(),
        seq_reservation: 1,
    })
    .expect("a fresh identity directory");
}

/// A forwarder that knows two nodes: verify the tag, learn the source address,
/// forward the bytes untouched.
async fn start_forwarder(keys: HashMap<NodeId, ForwarderKey>) -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let bindings: Arc<Mutex<HashMap<NodeId, SocketAddr>>> = Arc::new(Mutex::new(HashMap::new()));
    tokio::spawn(async move {
        let mut buffer = vec![0u8; 2048];
        loop {
            let Ok((len, from)) = socket.recv_from(&mut buffer).await else {
                return;
            };
            let datagram = &buffer[..len];
            let Ok(header) = OuterHeader::decode(datagram) else {
                continue;
            };
            let Some(key) = keys.get(&header.src) else {
                continue;
            };
            if !verify_tag(datagram, key) {
                continue;
            }
            let mut bindings = bindings.lock().await;
            bindings.insert(header.src, from);
            if let Some(destination) = bindings.get(&header.dst).copied() {
                // Verbatim: rewriting any field would break the AAD binding.
                let _ = socket.send_to(datagram, destination).await;
            }
        }
    });
    address
}

#[tokio::test]
async fn two_links_exchange_messages_through_a_forwarder() {
    let orchestrator_dir = tempfile::tempdir().unwrap();
    let host_dir = tempfile::tempdir().unwrap();
    let orchestrator_id = NodeId([0x11; 16]);
    let host_id = NodeId([0x22; 16]);
    let orchestrator_key = ForwarderKey([0x33; 32]);
    let host_key = ForwarderKey([0x44; 32]);
    let pair_key = PairKey::generate();

    let forwarder = start_forwarder(HashMap::from([
        (orchestrator_id, orchestrator_key.clone()),
        (host_id, host_key.clone()),
    ]))
    .await;

    enroll(
        orchestrator_dir.path(),
        orchestrator_id,
        host_id,
        Role::Orchestrator,
        pair_key.clone(),
        orchestrator_key,
        forwarder.to_string(),
    );
    enroll(
        host_dir.path(),
        host_id,
        orchestrator_id,
        Role::Host,
        pair_key,
        host_key,
        forwarder.to_string(),
    );

    let orchestrator = Link::connect(LinkConfig::new(orchestrator_dir.path()))
        .await
        .unwrap();
    let host = Link::connect(LinkConfig::new(host_dir.path()))
        .await
        .unwrap();

    orchestrator
        .send(host_id, b"dispatch task 1")
        .await
        .unwrap();
    let (from, _epoch, body) = tokio::time::timeout(PATIENCE, host.recv())
        .await
        .expect("the host heard nothing")
        .expect("the link closed");
    assert_eq!(from, orchestrator_id);
    assert_eq!(body, b"dispatch task 1");

    host.send(orchestrator_id, b"task 1 accepted")
        .await
        .unwrap();
    let (from, _epoch, body) = tokio::time::timeout(PATIENCE, orchestrator.recv())
        .await
        .expect("the orchestrator heard nothing")
        .expect("the link closed");
    assert_eq!(from, host_id);
    assert_eq!(body, b"task 1 accepted");

    host.send_screen_frame(orchestrator_id, b"latest screen frame")
        .await
        .unwrap();
    let (from, _epoch, body) = tokio::time::timeout(PATIENCE, orchestrator.recv())
        .await
        .expect("the orchestrator heard no screen update")
        .expect("the link closed");
    assert_eq!(from, host_id);
    assert_eq!(body, b"latest screen frame");

    let large_screen = vec![b'x'; 4_000];
    host.send_screen_frame(orchestrator_id, &large_screen)
        .await
        .unwrap();
    let (_from, _epoch, body) = tokio::time::timeout(PATIENCE, orchestrator.recv())
        .await
        .expect("the orchestrator heard no fragmented screen update")
        .expect("the link closed");
    assert_eq!(body, large_screen);

    // Both ends have heard from each other, so both report the peer live.
    let status = orchestrator.status();
    let peer = status.peer(&host_id).expect("a session for the host");
    assert_eq!(peer.liveness, Liveness::Live);
}

#[tokio::test]
async fn many_messages_arrive_in_order() {
    let orchestrator_dir = tempfile::tempdir().unwrap();
    let host_dir = tempfile::tempdir().unwrap();
    let orchestrator_id = NodeId([0x55; 16]);
    let host_id = NodeId([0x66; 16]);
    let orchestrator_key = ForwarderKey([0x77; 32]);
    let host_key = ForwarderKey([0x88; 32]);
    let pair_key = PairKey::generate();

    let forwarder = start_forwarder(HashMap::from([
        (orchestrator_id, orchestrator_key.clone()),
        (host_id, host_key.clone()),
    ]))
    .await;
    enroll(
        orchestrator_dir.path(),
        orchestrator_id,
        host_id,
        Role::Orchestrator,
        pair_key.clone(),
        orchestrator_key,
        forwarder.to_string(),
    );
    enroll(
        host_dir.path(),
        host_id,
        orchestrator_id,
        Role::Host,
        pair_key,
        host_key,
        forwarder.to_string(),
    );

    let orchestrator = Link::connect(LinkConfig::new(orchestrator_dir.path()))
        .await
        .unwrap();
    let host = Link::connect(LinkConfig::new(host_dir.path()))
        .await
        .unwrap();

    for index in 0..20u32 {
        orchestrator
            .send(host_id, format!("frame {index}").as_bytes())
            .await
            .unwrap();
    }
    for index in 0..20u32 {
        let (_, _epoch, body) = tokio::time::timeout(PATIENCE, host.recv())
            .await
            .expect("a frame went missing")
            .expect("the link closed");
        assert_eq!(body, format!("frame {index}").as_bytes());
    }
}

#[tokio::test]
async fn connecting_without_an_identity_is_reported_plainly() {
    let dir = tempfile::tempdir().unwrap();
    let error = Link::connect(LinkConfig::new(dir.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(error, medulla_link::LinkError::Key(_)),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn a_second_link_cannot_take_a_held_identity() {
    let dir = tempfile::tempdir().unwrap();
    let forwarder = start_forwarder(HashMap::new()).await;
    enroll(
        dir.path(),
        NodeId([0x99; 16]),
        NodeId([0xAA; 16]),
        Role::Host,
        PairKey::generate(),
        ForwarderKey([0xBB; 32]),
        forwarder.to_string(),
    );
    let _held = Link::connect(LinkConfig::new(dir.path())).await.unwrap();
    let error = Link::connect(LinkConfig::new(dir.path()))
        .await
        .unwrap_err();
    assert!(
        matches!(error, medulla_link::LinkError::Key(keys::KeyError::Busy(_))),
        "unexpected error: {error}"
    );
}
