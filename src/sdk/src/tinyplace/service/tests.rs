//! Tests for the service module.

use super::*;
use crate::config::Peer;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn merge_into_overlays_identity_dedups_roster_and_upserts_presence() {
    use crate::runtime::{AgentDescriptor, AgentPresence, RuntimeSnapshot};

    let mut snapshot = RuntimeSnapshot {
        roster: vec![AgentDescriptor {
            id: "peer-1".into(),
            ..Default::default()
        }],
        ..Default::default()
    };

    // A duplicate id (peer-1) must not be appended twice; peer-2 is new.
    let mut obs = TinyplaceObservation {
        roster: vec![
            AgentDescriptor {
                id: "peer-1".into(),
                ..Default::default()
            },
            AgentDescriptor {
                id: "peer-2".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    obs.presence
        .insert("peer-1".into(), AgentPresence::default());

    obs.merge_into(&mut snapshot);

    let ids: Vec<&str> = snapshot.roster.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["peer-1", "peer-2"]);
    assert!(snapshot.presence.contains_key("peer-1"));
}

#[test]
fn endpoint_prefers_env_override_over_tui_base_url() {
    let tp = TinyplaceFileConfig::default();
    // An env override resolves to something other than the DEFAULT_ENDPOINT,
    // so it wins over the TUI base_url.
    let e = env(&[("TINYPLACE_ENDPOINT", "https://override")]);
    assert_eq!(
        resolve_endpoint_with_config(&e, &tp, "https://tui-default"),
        "https://override"
    );
}

#[test]
fn endpoint_falls_back_to_tui_base_url_then_default() {
    let tp = TinyplaceFileConfig::default();
    // No env/config endpoint → resolver returns DEFAULT_ENDPOINT, so the
    // explicit TUI base_url is used.
    assert_eq!(
        resolve_endpoint_with_config(&HashMap::new(), &tp, "https://tui"),
        "https://tui"
    );
    // Empty TUI base_url → the DEFAULT_ENDPOINT stands.
    assert_eq!(
        resolve_endpoint_with_config(&HashMap::new(), &tp, ""),
        crate::tinyplace::DEFAULT_ENDPOINT
    );
}

#[test]
fn roster_from_bare_peer_uses_id_fallbacks() {
    let config = TinyplaceConfig {
        peers: vec![Peer {
            id: "peer-x".to_string(),
            name: None,
            handle: None,
            address: None,
            tags: None,
            description: None,
            protocol: "task".to_string(),
        }],
        ..Default::default()
    };
    let roster = roster_from_peers(&config);
    assert_eq!(roster.len(), 1);
    let d = &roster[0];
    // name falls back to the id when neither name nor handle is set.
    assert_eq!(d.name, "peer-x");
    assert_eq!(d.description, "");
    assert!(d.tags.is_empty());
    // harness tag is always present; no handle/address keys for a bare peer.
    assert_eq!(
        d.metadata.get("harness").and_then(|v| v.as_str()),
        Some("tinyplace")
    );
    assert!(d.metadata.get("handle").is_none());
    assert!(d.metadata.get("address").is_none());
}

#[test]
fn roster_from_peer_prefers_handle_when_name_absent() {
    let config = TinyplaceConfig {
        peers: vec![Peer {
            id: "peer-y".to_string(),
            name: None,
            handle: Some("@handle".to_string()),
            address: Some("addr".to_string()),
            tags: None,
            description: None,
            protocol: "task".to_string(),
        }],
        ..Default::default()
    };
    let roster = roster_from_peers(&config);
    assert_eq!(roster[0].name, "@handle");
    assert_eq!(
        roster[0].metadata.get("handle").and_then(|v| v.as_str()),
        Some("@handle")
    );
    assert_eq!(
        roster[0].metadata.get("address").and_then(|v| v.as_str()),
        Some("addr")
    );
}

#[test]
fn now_ms_is_positive() {
    assert!(now_ms() > 0);
}
