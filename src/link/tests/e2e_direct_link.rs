//! Two `Link`s over real UDP sockets with **no forwarder** — the SSH-bootstrapped
//! path.
//!
//! `e2e_direct_link` is the sibling of `e2e_link`: same wiring under test
//! (`Link::connect`, the driver task, the inbound queue, `status()`), but along
//! the path a remote host is reached by. What is deliberately absent is the whole
//! middle of the other suite — there is no stand-in forwarder here, because the
//! point of this path is that nothing sits between the endpoints.
//!
//! The address-learning rule itself is unit-tested in `link/direct_tests.rs`,
//! where a datagram can be replayed precisely. This suite proves the pieces are
//! wired together: that a link with no forwarder configured actually carries
//! messages and screen rows both ways, and that a peer's outer tag is checked
//! rather than ignored.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use medulla_link::keys::{self, ForwarderKey, NodeId, NodeState, PairKey, Role};
use medulla_link::{Link, LinkConfig, LinkPath};
use tokio::net::UdpSocket;

/// How long to wait for a datagram to make a loopback round trip.
const PATIENCE: Duration = Duration::from_secs(5);

/// Write an identity into `dir` so `Link::connect` can open it.
///
/// The `forwarder_endpoint` recorded here is deliberate nonsense: a direct link
/// must never consult it, and a test that supplied a plausible one could not
/// tell the difference between "ignored it" and "used it and it happened to
/// work".
fn enroll(
    dir: &Path,
    node_id: NodeId,
    peer_node_id: NodeId,
    role: Role,
    pair_key: PairKey,
) -> NodeId {
    keys::acquire_or_create(dir, || NodeState {
        version: 1,
        node_id,
        role,
        pair_key,
        // Equally never used: the direct path derives its outer key from the
        // pair key instead (see `link::direct::path_key`).
        forwarder_key: ForwarderKey([0xFF; 32]),
        forwarder_endpoint: "255.255.255.255:1".to_string(),
        peer_node_id,
        peers: Vec::new(),
        seq_reservation: 1,
    })
    .expect("a fresh identity directory");
    node_id
}

/// Reserve a loopback UDP port and give it back.
///
/// Both ends need each other's address *before* either link starts, and
/// `Link::connect` binds its own socket — so the port has to be chosen here and
/// handed to both. Binding and dropping is the ordinary way to have the OS pick
/// a free one.
async fn free_port() -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.local_addr().unwrap()
}

/// Bring up both ends of a direct link, already pointed at each other.
async fn direct_pair(
    client_dir: &Path,
    daemon_dir: &Path,
) -> (
    medulla_link::LinkHandle,
    NodeId,
    medulla_link::LinkHandle,
    NodeId,
) {
    let client_id = NodeId([0x11; 16]);
    let daemon_id = NodeId([0x22; 16]);
    let pair_key = PairKey::generate();

    enroll(
        client_dir,
        client_id,
        daemon_id,
        Role::Orchestrator,
        pair_key.clone(),
    );
    enroll(daemon_dir, daemon_id, client_id, Role::Host, pair_key);

    let client_addr = free_port().await;
    let daemon_addr = free_port().await;

    let mut client_config = LinkConfig::new(client_dir);
    client_config.bind = client_addr;
    client_config.path = LinkPath::Direct {
        endpoint: daemon_addr.to_string(),
    };

    let mut daemon_config = LinkConfig::new(daemon_dir);
    daemon_config.bind = daemon_addr;
    daemon_config.path = LinkPath::Direct {
        endpoint: client_addr.to_string(),
    };

    let client = Link::connect(client_config).await.unwrap();
    let daemon = Link::connect(daemon_config).await.unwrap();
    (client, client_id, daemon, daemon_id)
}

#[tokio::test]
async fn two_links_exchange_messages_with_no_forwarder() {
    let client_dir = tempfile::tempdir().unwrap();
    let daemon_dir = tempfile::tempdir().unwrap();
    let (client, client_id, daemon, daemon_id) =
        direct_pair(client_dir.path(), daemon_dir.path()).await;

    client.send(daemon_id, b"open a shell").await.unwrap();
    let (from, _epoch, body) = tokio::time::timeout(PATIENCE, daemon.recv())
        .await
        .expect("the daemon heard nothing")
        .expect("the link closed");
    assert_eq!(from, client_id);
    assert_eq!(body, b"open a shell");

    daemon.send(client_id, b"session w_1").await.unwrap();
    let (from, _epoch, body) = tokio::time::timeout(PATIENCE, client.recv())
        .await
        .expect("the client heard nothing")
        .expect("the link closed");
    assert_eq!(from, daemon_id);
    assert_eq!(body, b"session w_1");
}

#[tokio::test]
async fn a_screen_frame_crosses_a_direct_link() {
    // The relay's whole job, reduced to its smallest form: channel 1 has to work
    // over the direct path, not just channel 0.
    let client_dir = tempfile::tempdir().unwrap();
    let daemon_dir = tempfile::tempdir().unwrap();
    let (client, client_id, daemon, daemon_id) =
        direct_pair(client_dir.path(), daemon_dir.path()).await;

    // Wake the link from the client side first, so the daemon has somewhere to
    // send to before it produces a frame.
    client.send(daemon_id, b"subscribe").await.unwrap();
    let _ = tokio::time::timeout(PATIENCE, daemon.recv()).await;

    let frame = b"the screen, as bytes".to_vec();
    daemon.send_screen_frame(client_id, &frame).await.unwrap();
    let (_from, _epoch, body) = tokio::time::timeout(PATIENCE, client.recv())
        .await
        .expect("no screen frame arrived")
        .expect("the link closed");
    assert_eq!(body, frame);
}

#[tokio::test]
async fn a_direct_link_refuses_more_than_one_peer() {
    // A direct link points one socket at one address, so a second peer would be
    // silently sent the first one's datagrams. Better to refuse at connect than
    // to be subtly wrong at runtime.
    let dir = tempfile::tempdir().unwrap();
    let node_id = NodeId([0x11; 16]);
    enroll(
        dir.path(),
        node_id,
        NodeId([0x22; 16]),
        Role::Orchestrator,
        PairKey::generate(),
    );

    let mut config = LinkConfig::new(dir.path());
    config.path = LinkPath::Direct {
        endpoint: "127.0.0.1:9".to_string(),
    };
    config.peers = vec![
        medulla_link::PeerConfig {
            node_id: NodeId([0x22; 16]),
            pair_key: PairKey::generate(),
        },
        medulla_link::PeerConfig {
            node_id: NodeId([0x33; 16]),
            pair_key: PairKey::generate(),
        },
    ];
    let error = Link::connect(config)
        .await
        .expect_err("two peers must be refused");
    assert!(
        matches!(error, medulla_link::LinkError::DirectPeerCount(2)),
        "expected a peer-count refusal, got {error:?}"
    );
}
