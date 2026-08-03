//! Unit tests for the host-link observation service.
//!
//! `LinkService::start` needs a real link identity and a UDP socket, so what is
//! tested here is the pure projection: node-id parsing, roster construction, and
//! the snapshot merge.

use super::*;
use crate::config::Peer;

/// A configured peer with the given id and optional node id.
fn peer(id: &str, node_id: Option<&str>) -> Peer {
    Peer {
        id: id.to_string(),
        node_id: node_id.map(str::to_string),
        name: Some(format!("Peer {id}")),
        handle: None,
        address: None,
        tags: None,
        description: None,
        protocol: "medulla-task/1".to_string(),
    }
}

#[test]
fn a_node_id_parses_only_from_thirty_two_hex_characters() {
    let id = parse_node_id("000102030405060708090a0b0c0d0e0f").expect("32 hex characters parse");
    assert_eq!(id.0[0], 0);
    assert_eq!(id.0[15], 0x0f);

    // A short, long, or non-hex value is refused rather than silently padded —
    // a wrong node id addresses a datagram at nobody.
    assert!(parse_node_id("00").is_none());
    assert!(parse_node_id("000102030405060708090a0b0c0d0e0f00").is_none());
    assert!(parse_node_id("zz0102030405060708090a0b0c0d0e0f").is_none());
}

#[test]
fn the_roster_projects_every_configured_peer_and_tags_it_link() {
    let config = LinkConfig {
        peers: vec![peer("h1", None), peer("h2", Some("00".repeat(16).as_str()))],
        ..LinkConfig::default()
    };
    let roster = roster_from_peers(&config);
    assert_eq!(roster.len(), 2);
    assert_eq!(roster[0].id, "h1");
    assert_eq!(roster[0].name, "Peer h1");
    assert_eq!(roster[0].metadata["harness"], "link");
}

#[test]
fn enrolled_nodes_without_config_rows_receive_fallback_roster_entries() {
    let node = NodeId([0x42; 16]);
    let roster = roster_from_peers_and_nodes(&LinkConfig::default(), &[(node, node.to_string())]);

    assert_eq!(roster.len(), 1);
    assert_eq!(roster[0].id, node.to_string());
    assert_eq!(roster[0].name, node.to_string());
    assert_eq!(roster[0].metadata["harness"], "link");
}

#[test]
fn merging_overlays_identity_dedupes_the_roster_and_upserts_presence() {
    let mut snapshot = crate::runtime::RuntimeSnapshot {
        roster: roster_from_peers(&LinkConfig {
            peers: vec![peer("h1", None)],
            ..LinkConfig::default()
        }),
        ..Default::default()
    };
    let observation = LinkObservation {
        identity: Some(LinkIdentity {
            node_name: "orchestrator".into(),
            forwarder: "f:1".into(),
        }),
        roster: roster_from_peers(&LinkConfig {
            peers: vec![peer("h1", None), peer("h2", None)],
            ..LinkConfig::default()
        }),
        presence: HashMap::from([(
            "h1".to_string(),
            AgentPresence {
                online: true,
                detail: Some("live".into()),
                at: 7,
            },
        )]),
        notice: None,
    };

    observation.merge_into(&mut snapshot);

    assert_eq!(
        snapshot.link.as_ref().map(|id| id.node_name.as_str()),
        Some("orchestrator")
    );
    // `h1` was already there, so only `h2` is appended.
    assert_eq!(snapshot.roster.len(), 2);
    assert!(snapshot.presence["h1"].online);
}
