//! Unit tests for bridge-level fragmentation and reassembly.

use super::*;

fn chunk(id: u64, index: u32, count: u32, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.extend_from_slice(CHUNK_MAGIC);
    frame.extend_from_slice(&id.to_be_bytes());
    frame.extend_from_slice(&index.to_be_bytes());
    frame.extend_from_slice(&count.to_be_bytes());
    frame.extend_from_slice(body);
    frame
}

#[tokio::test]
async fn fragments_reassemble_without_mixing_interleaved_messages() {
    let inbox = Inbox::default();
    let peer = NodeId([7; 16]);
    let other_peer = NodeId([8; 16]);

    assert!(reassemble(peer, 1, chunk(1, 0, 2, b"large "), &inbox)
        .await
        .is_none());
    assert!(reassemble(other_peer, 1, chunk(2, 0, 2, b"other "), &inbox)
        .await
        .is_none());
    assert_eq!(
        reassemble(peer, 1, chunk(1, 1, 2, b"frame"), &inbox).await,
        Some(b"large frame".to_vec())
    );
    assert_eq!(
        reassemble(other_peer, 1, chunk(2, 1, 2, b"message"), &inbox).await,
        Some(b"other message".to_vec())
    );
}

#[tokio::test]
async fn unframed_payloads_remain_compatible() {
    let inbox = Inbox::default();
    let body = b"legacy frame".to_vec();
    assert_eq!(
        reassemble(NodeId([8; 16]), 1, body.clone(), &inbox).await,
        Some(body)
    );
}

#[tokio::test]
async fn a_new_first_fragment_replaces_a_cancelled_frame_in_the_same_epoch() {
    let inbox = Inbox::default();
    let peer = NodeId([9; 16]);
    assert!(reassemble(peer, 1, chunk(1, 0, 2, b"preserved "), &inbox)
        .await
        .is_none());
    assert!(reassemble(peer, 1, chunk(2, 0, 2, b"new"), &inbox)
        .await
        .is_none());
    assert_eq!(
        reassemble(peer, 1, chunk(2, 1, 2, b" frame"), &inbox).await,
        Some(b"new frame".to_vec())
    );
}

#[tokio::test]
async fn a_new_sender_epoch_replaces_an_abandoned_partial() {
    let inbox = Inbox::default();
    let peer = NodeId([11; 16]);
    assert!(reassemble(peer, 1, chunk(1, 0, 2, b"abandoned"), &inbox)
        .await
        .is_none());
    assert!(reassemble(peer, 2, chunk(2, 0, 2, b"new "), &inbox)
        .await
        .is_none());
    assert_eq!(
        reassemble(peer, 2, chunk(2, 1, 2, b"frame"), &inbox).await,
        Some(b"new frame".to_vec())
    );
}

#[tokio::test]
async fn incomplete_reassembly_survives_a_long_gap() {
    let inbox = Inbox::default();
    let peer = NodeId([10; 16]);
    assert!(reassemble(peer, 1, chunk(1, 0, 2, b"held "), &inbox)
        .await
        .is_none());
    assert_eq!(
        reassemble(peer, 1, chunk(1, 1, 2, b"frame"), &inbox).await,
        Some(b"held frame".to_vec())
    );
}

#[test]
fn fragment_message_ids_are_random_across_calls() {
    assert_ne!(next_message_id(), next_message_id());
}

#[test]
fn a_friendly_peer_name_preserves_the_enrolled_node_id_alias() {
    let node_id = NodeId([12; 16]);
    let aliases = peer_aliases(&LinkPeer {
        name: "worker-alpha".to_string(),
        node_id,
    });

    assert_eq!(aliases[0], "worker-alpha");
    assert_eq!(aliases[1], node_id.to_string());
}
