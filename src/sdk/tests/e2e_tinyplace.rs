//! End-to-end tests for the tiny.place background service and runtime loops
//! against an in-test mock tiny.place HTTP API. No real network: the mock stands
//! in for the relay/directory/presence backend.

#[path = "support/mock_tinyplace.rs"]
mod mock_tinyplace;

use std::time::Duration;

use medulla::config::{Peer, TinyplaceConfig};
use medulla::tinyplace::frames::{encode_task_frame, EncodeFrameInput, TaskFrameKind};
use medulla::tinyplace::service::TinyplaceService;
use medulla::tinyplace::{
    spawn_contact_auto_accepter, spawn_mailbox_poll, spawn_presence_heartbeat,
};
use tinyplace::types::MessageEnvelope;
use tinyplace::{TinyPlaceClient, TinyPlaceClientOptions};

use mock_tinyplace::{wait_until, MockConfig, MockMessage, MockTinyplace, PendingRequest};

const T: Duration = Duration::from_secs(6);

/// A unique temp identity dir for a service under test.
fn temp_identity_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "tinyplace-e2e-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn signerless_client(base_url: &str) -> TinyPlaceClient {
    TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url: base_url.to_string(),
        ..Default::default()
    })
}

fn peer(id: &str) -> Peer {
    Peer {
        id: id.to_string(),
        name: Some(format!("Peer {id}")),
        handle: Some(format!("@{id}")),
        address: Some(format!("addr-{id}")),
        tags: Some(vec!["tinyplace".to_string()]),
        description: Some("a peer".to_string()),
        protocol: "task".to_string(),
    }
}

fn tp_config(
    base_url: &str,
    dir: &std::path::Path,
    peers: Vec<Peer>,
    accept: &str,
) -> TinyplaceConfig {
    TinyplaceConfig {
        base_url: base_url.to_string(),
        identity_dir: dir.to_string_lossy().to_string(),
        handle: Some("@me".to_string()),
        display_name: Some("Me".to_string()),
        bio: None,
        auto_discover_peers: false,
        accept_contacts: accept.to_string(),
        peers,
    }
}

/// Service start mints an identity, seeds the roster, heartbeats, auto-accepts a
/// configured peer's pending request, and fills presence into the observation.
#[tokio::test]
async fn service_start_fills_observation_and_accepts_peer() {
    let mock = MockTinyplace::start(MockConfig {
        pending: vec![PendingRequest::incoming("peer-1")],
        online: vec!["peer-1".to_string()],
        ..Default::default()
    })
    .await;
    let dir = temp_identity_dir("service");

    let config = tp_config(&mock.base_url, &dir, vec![peer("peer-1")], "peers");
    let service = TinyplaceService::start(&config).expect("service starts");
    let observation = service.observation();

    // Identity minted + roster seeded immediately.
    {
        let obs = observation.lock().unwrap();
        let identity = obs.identity.as_ref().expect("identity present");
        assert!(!identity.agent_id.is_empty());
        assert!(!identity.public_key.is_empty());
        assert_eq!(identity.handle.as_deref(), Some("@me"));
        assert_eq!(obs.roster.len(), 1);
        assert_eq!(obs.roster[0].id, "peer-1");
        assert_eq!(
            obs.roster[0]
                .metadata
                .get("harness")
                .and_then(|v| v.as_str()),
            Some("tinyplace")
        );
    }

    // Presence poll fills the observation for the configured peer.
    wait_until("presence recorded", T, || {
        observation
            .lock()
            .unwrap()
            .presence
            .get("peer-1")
            .map(|p| p.online)
            .unwrap_or(false)
    })
    .await;

    // Heartbeat + contact accept both hit the server.
    wait_until("heartbeat sent", T, || mock.heartbeats() >= 1).await;
    wait_until("peer accepted", T, || {
        mock.accepted().iter().any(|id| id == "peer-1")
    })
    .await;

    // The identity was persisted to the tempdir config file.
    assert!(dir.join("config.json").exists());

    drop(service);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Auto-accept is a fail-closed allowlist: with `accept_contacts = "peers"` only
/// configured peers are accepted; an unknown requester stays pending.
#[tokio::test]
async fn service_auto_accept_allowlist_rejects_unknown() {
    let mock = MockTinyplace::start(MockConfig {
        pending: vec![
            PendingRequest::incoming("peer-1"),
            PendingRequest::incoming("stranger"),
        ],
        online: vec![],
        ..Default::default()
    })
    .await;
    let dir = temp_identity_dir("allowlist");

    let config = tp_config(&mock.base_url, &dir, vec![peer("peer-1")], "peers");
    let service = TinyplaceService::start(&config).expect("service starts");

    wait_until("configured peer accepted", T, || {
        mock.accepted().iter().any(|id| id == "peer-1")
    })
    .await;
    // Give the loop a couple more ticks; the stranger must never be accepted.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !mock.accepted().iter().any(|id| id == "stranger"),
        "stranger must not be auto-accepted under the allowlist"
    );

    drop(service);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `accept_contacts = "all"` accepts any requester, including unconfigured ones.
#[tokio::test]
async fn service_accept_all_accepts_stranger() {
    let mock = MockTinyplace::start(MockConfig {
        pending: vec![PendingRequest::incoming("stranger")],
        ..Default::default()
    })
    .await;
    let dir = temp_identity_dir("accept-all");

    let config = tp_config(&mock.base_url, &dir, vec![], "all");
    let service = TinyplaceService::start(&config).expect("service starts");

    wait_until("stranger accepted", T, || {
        mock.accepted().iter().any(|id| id == "stranger")
    })
    .await;

    drop(service);
    let _ = std::fs::remove_dir_all(&dir);
}

/// With no configured peers the presence poll loop is not spawned, but the
/// heartbeat still runs.
#[tokio::test]
async fn service_without_peers_skips_presence_poll() {
    let mock = MockTinyplace::start(MockConfig::default()).await;
    let dir = temp_identity_dir("nopeers");

    let config = tp_config(&mock.base_url, &dir, vec![], "peers");
    let service = TinyplaceService::start(&config).expect("service starts");
    let observation = service.observation();

    wait_until("heartbeat sent", T, || mock.heartbeats() >= 1).await;
    // No peers → no presence queries, empty roster + presence.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(mock.presence_queries(), 0);
    {
        let obs = observation.lock().unwrap();
        assert!(obs.roster.is_empty());
        assert!(obs.presence.is_empty());
    }

    drop(service);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A reused-identity start: an existing config seed yields a stable agent id.
#[tokio::test]
async fn service_reuses_existing_identity() {
    let mock = MockTinyplace::start(MockConfig::default()).await;
    let dir = temp_identity_dir("reuse");
    let config = tp_config(&mock.base_url, &dir, vec![], "peers");

    let first = TinyplaceService::start(&config).expect("first start");
    let agent = first
        .observation()
        .lock()
        .unwrap()
        .identity
        .clone()
        .unwrap()
        .agent_id;
    drop(first);

    let second = TinyplaceService::start(&config).expect("second start");
    let agent2 = second
        .observation()
        .lock()
        .unwrap()
        .identity
        .clone()
        .unwrap()
        .agent_id;
    assert_eq!(agent, agent2, "identity is stable across restarts");

    drop(second);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The mailbox poll destructively reads messages, decodes task frames, skips
/// messages the decode hook drops, and acknowledges every delivered message.
#[tokio::test]
async fn mailbox_poll_decodes_skips_and_acknowledges() {
    let frame_body = encode_task_frame(EncodeFrameInput {
        kind: TaskFrameKind::Task,
        task_id: "t-1".to_string(),
        text: "do the thing".to_string(),
        ts: "2026-07-18T00:00:00Z".to_string(),
        correlation_id: Some("corr-1".to_string()),
        harness: None,
        provider: None,
        model: None,
    });

    let mock = MockTinyplace::start(MockConfig {
        messages: vec![
            MockMessage {
                id: "m-frame".to_string(),
                from: "peer-1".to_string(),
                to: "@me".to_string(),
                body: frame_body.clone(),
            },
            MockMessage {
                id: "m-garbage".to_string(),
                from: "peer-1".to_string(),
                to: "@me".to_string(),
                body: "not a frame".to_string(),
            },
            MockMessage {
                id: "m-skip".to_string(),
                from: "peer-1".to_string(),
                to: "@me".to_string(),
                body: "SKIP".to_string(),
            },
        ],
        ..Default::default()
    })
    .await;

    let client = signerless_client(&mock.base_url);
    // decode_body: pass the plaintext through, but drop anything marked SKIP.
    let mut poll = spawn_mailbox_poll(
        client,
        "@me".to_string(),
        Duration::from_millis(15),
        50,
        |env: &MessageEnvelope| {
            if env.body == "SKIP" {
                None
            } else {
                Some(env.body.clone())
            }
        },
    );

    // The valid frame and the garbage message both surface; the skipped one does not.
    let first = tokio::time::timeout(T, poll.receiver.recv())
        .await
        .expect("recv 1 in time")
        .expect("item 1");
    let second = tokio::time::timeout(T, poll.receiver.recv())
        .await
        .expect("recv 2 in time")
        .expect("item 2");

    let mut items = [first, second];
    items.sort_by(|a, b| a.envelope.id.cmp(&b.envelope.id));
    // m-frame decodes to a task frame; m-garbage decodes to no frame.
    let frame_item = items.iter().find(|i| i.envelope.id == "m-frame").unwrap();
    assert!(frame_item.frame.is_some());
    assert_eq!(frame_item.frame.as_ref().unwrap().task_id, "t-1");
    let garbage_item = items.iter().find(|i| i.envelope.id == "m-garbage").unwrap();
    assert!(garbage_item.frame.is_none());
    assert_eq!(garbage_item.body, "not a frame");

    // Every delivered message is acknowledged (destructive read), skipped included.
    wait_until("all three acknowledged", T, || {
        let acked = mock.acknowledged();
        acked.contains(&"m-frame".to_string())
            && acked.contains(&"m-garbage".to_string())
            && acked.contains(&"m-skip".to_string())
    })
    .await;

    poll.handle.abort();
}

/// The presence heartbeat loop hits the server repeatedly and keeps running even
/// when the server returns 5xx.
#[tokio::test]
async fn presence_heartbeat_repeats_and_tolerates_errors() {
    let mock = MockTinyplace::start(MockConfig {
        heartbeat_status: 500,
        ..Default::default()
    })
    .await;

    let client = signerless_client(&mock.base_url);
    let handle = spawn_presence_heartbeat(client, Duration::from_millis(15));

    // Despite every heartbeat 500ing, the loop keeps beating.
    wait_until("heartbeats repeat under errors", T, || {
        mock.heartbeats() >= 3
    })
    .await;

    // Recovery: flip to 200 and confirm it keeps beating.
    mock.configure(|c| c.heartbeat_status = 200);
    let before = mock.heartbeats();
    wait_until("heartbeats continue after recovery", T, || {
        mock.heartbeats() > before
    })
    .await;

    handle.abort();
}

/// The standalone contact auto-accepter honours its allow predicate, skips empty
/// ids, and leaves rejected requests pending.
#[tokio::test]
async fn contact_auto_accepter_honours_allow_predicate() {
    let mock = MockTinyplace::start(MockConfig {
        pending: vec![
            PendingRequest::incoming("@ok"),
            PendingRequest::incoming("@no"),
            PendingRequest::incoming(""), // empty id → skipped via the continue branch
        ],
        ..Default::default()
    })
    .await;

    let client = signerless_client(&mock.base_url);
    let handle =
        spawn_contact_auto_accepter(client, Duration::from_millis(15), |id: &str| id == "@ok");

    wait_until("@ok accepted", T, || {
        mock.accepted().iter().any(|id| id == "@ok")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(!mock.accepted().iter().any(|id| id == "@no"));
    assert!(!mock.accepted().iter().any(|id| id.is_empty()));

    handle.abort();
}

/// The service publishes Signal pre-keys on startup.
///
/// Without a published bundle a peer cannot run X3DH against this identity, so
/// every DM to it fails to establish a session — the agent shows up in the
/// directory but silently receives nothing. The headless daemon has always done
/// this during onboarding; anything else that holds an identity must too.
#[tokio::test]
async fn service_publishes_pre_keys_so_peers_can_reach_it() {
    let mock = MockTinyplace::start(MockConfig::default()).await;
    let dir = temp_identity_dir("prekeys");

    let config = tp_config(&mock.base_url, &dir, vec![], "peers");
    let service = TinyplaceService::start(&config).expect("service starts");

    // The publish is what makes this identity reachable: without a bundle a peer
    // cannot run X3DH against it, so every DM fails to establish a session and
    // the agent silently receives nothing. The mock serves no key routes, so the
    // call cannot succeed — what matters, and what was missing entirely, is that
    // it is attempted at all.
    wait_until("pre-key publish attempted", T, || {
        mock.requests().iter().any(|r| r.path.contains("/keys/"))
    })
    .await;

    // And the failure is reported rather than swallowed: an agent nobody can
    // reach must not look identical to one that simply has no mail.
    wait_until("failure surfaced", T, || {
        service.observation().lock().unwrap().notice.is_some()
    })
    .await;
    let notice = service
        .observation()
        .lock()
        .unwrap()
        .notice
        .clone()
        .expect("a failed publish leaves a notice");
    assert!(notice.contains("pre-key"), "got: {notice}");

    drop(service);
    let _ = std::fs::remove_dir_all(&dir);
}
