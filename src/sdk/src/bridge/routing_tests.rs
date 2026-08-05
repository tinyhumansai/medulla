//! Unit tests for [`RoutingBridge`]: which side of the router an address lands
//! on, and that the local side never reaches the remote transport.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::bridge::InboundMessage;
use crate::bridge::{Bridge, LocalBridgeNetwork, RoutingBridge};

/// A remote stand-in that records every call it receives.
///
/// The assertions that matter are negative — "the local path did NOT touch the
/// remote" — so the double records rather than merely responding.
#[derive(Default)]
struct RecordingRemote {
    sent: Mutex<Vec<(String, String)>>,
    resets: Mutex<Vec<String>>,
    inbox: Mutex<Vec<InboundMessage>>,
}

impl RecordingRemote {
    fn with_inbox(messages: Vec<InboundMessage>) -> Self {
        Self {
            inbox: Mutex::new(messages),
            ..Default::default()
        }
    }

    fn sent(&self) -> Vec<(String, String)> {
        self.sent.lock().unwrap().clone()
    }
}

#[async_trait]
impl Bridge for RecordingRemote {
    async fn send(&self, to: &str, body: &str) -> Result<(), String> {
        self.sent
            .lock()
            .unwrap()
            .push((to.to_string(), body.to_string()));
        Ok(())
    }

    async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage> {
        let mut inbox = self.inbox.lock().unwrap();
        let count = (limit.max(0) as usize).min(inbox.len());
        inbox.drain(..count).collect()
    }

    /// Answers every address as down, so a caller that reaches this at all is
    /// distinguishable from one that silently took the trait's default.
    async fn presence(&self, addresses: &[String]) -> std::collections::HashMap<String, bool> {
        addresses.iter().map(|a| (a.clone(), false)).collect()
    }

    async fn resolve_handle(&self, name: &str) -> Option<String> {
        (name == "@remote").then(|| "remote-address".to_string())
    }

    async fn reset_session(&self, peer: &str) {
        self.resets.lock().unwrap().push(peer.to_string());
    }
}

/// A router bound as `hub` with a `host` endpoint beside it, plus the remote it
/// falls through to.
fn wired() -> (
    RoutingBridge,
    Arc<RecordingRemote>,
    crate::bridge::LocalBridge,
) {
    let network = LocalBridgeNetwork::new();
    let hub = network.bind("hub").unwrap();
    let host = network.bind("host").unwrap();
    let remote = Arc::new(RecordingRemote::default());
    let router = RoutingBridge::new(hub, remote.clone() as Arc<dyn Bridge>);
    (router, remote, host)
}

#[tokio::test]
async fn a_bound_local_address_is_delivered_in_process_and_never_sent_remotely() {
    let (router, remote, host) = wired();

    router.send("host", "run this here").await.unwrap();

    assert_eq!(
        host.drain_inbox(10)
            .await
            .into_iter()
            .map(|m| (m.from, m.text))
            .collect::<Vec<_>>(),
        vec![("hub".to_string(), "run this here".to_string())]
    );
    assert!(remote.sent().is_empty());
}

#[tokio::test]
async fn an_unbound_address_falls_through_to_the_remote_transport() {
    let (router, remote, _host) = wired();

    router
        .send("remote-address", "run this there")
        .await
        .unwrap();

    assert_eq!(
        remote.sent(),
        vec![("remote-address".to_string(), "run this there".to_string())]
    );
}

#[tokio::test]
async fn an_endpoint_that_goes_away_stops_routing_locally() {
    let (router, remote, host) = wired();
    assert!(router.is_local("host").await);

    drop(host);

    assert!(!router.is_local("host").await);
    router.send("host", "after teardown").await.unwrap();
    assert_eq!(
        remote.sent(),
        vec![("host".to_string(), "after teardown".to_string())]
    );
}

#[tokio::test]
async fn draining_merges_both_inboxes_within_one_shared_limit() {
    let network = LocalBridgeNetwork::new();
    let hub = network.bind("hub").unwrap();
    let host = network.bind("host").unwrap();
    let remote = Arc::new(RecordingRemote::with_inbox(vec![
        InboundMessage {
            from: "peer".to_string(),
            text: "remote-one".to_string(),
        },
        InboundMessage {
            from: "peer".to_string(),
            text: "remote-two".to_string(),
        },
    ]));
    let router = RoutingBridge::new(hub, remote.clone() as Arc<dyn Bridge>);
    host.send("hub", "local-one").await.unwrap();

    let drained = router.drain_inbox(2).await;

    assert_eq!(
        drained.into_iter().map(|m| m.text).collect::<Vec<_>>(),
        vec!["local-one".to_string(), "remote-one".to_string()]
    );
    // The over-limit remote frame is still queued rather than dropped.
    assert_eq!(remote.drain_inbox(10).await.len(), 1);
}

#[tokio::test]
async fn a_local_peer_needs_no_session_reset() {
    let (router, remote, _host) = wired();

    assert!(router.is_device_local("host").await);
    router.reset_session("host").await;

    assert!(remote.resets.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_remote_peer_keeps_its_session_reset() {
    let (router, remote, _host) = wired();

    assert!(!router.is_device_local("remote-address").await);
    router.reset_session("remote-address").await;

    assert_eq!(*remote.resets.lock().unwrap(), vec!["remote-address"]);
}

#[tokio::test]
async fn handles_resolve_locally_first_then_remotely() {
    let network = LocalBridgeNetwork::new();
    let hub = network.bind("hub").unwrap();
    let _host = network.bind("@host").unwrap();
    let router = RoutingBridge::new(hub, Arc::new(RecordingRemote::default()) as Arc<dyn Bridge>);

    assert_eq!(
        router.resolve_handle("host").await.as_deref(),
        Some("@host")
    );
    assert_eq!(
        router.resolve_handle("@remote").await.as_deref(),
        Some("remote-address")
    );
    assert_eq!(router.resolve_handle("@nobody").await, None);
}

#[tokio::test]
async fn without_a_remote_a_non_local_address_fails_loudly() {
    let network = LocalBridgeNetwork::new();
    let router = RoutingBridge::local_only(network.bind("hub").unwrap());

    let error = router.send("somewhere-else", "hello").await.unwrap_err();

    assert!(
        error.contains("somewhere-else"),
        "unexpected error: {error}"
    );
    assert!(router.drain_inbox(10).await.is_empty());
    assert_eq!(router.local_address(), "hub");
    assert!(router.remote().is_none());
}

#[tokio::test]
async fn presence_is_asked_of_the_remote() {
    // The bug this exists to catch: `RoutingBridge` implements `Bridge`, so a
    // method it forgets to forward silently takes the trait's default instead of
    // reaching the transport that can actually answer. That default is "no
    // opinion", which reads as a healthy fleet — so the hub advertised a worker
    // that the roster knew was down, and nothing anywhere said otherwise.
    let network = LocalBridgeNetwork::new();
    let hub = network.bind("presence-hub").unwrap();
    let remote = Arc::new(RecordingRemote::default());
    let bridge = RoutingBridge::new(hub, remote as Arc<dyn Bridge>);

    let answered = bridge.presence(&["GRVaddr".to_string()]).await;

    assert_eq!(
        answered.get("GRVaddr"),
        Some(&false),
        "the remote's answer must come through, not the trait default"
    );
}

#[tokio::test]
async fn a_local_only_bridge_has_nobody_to_ask_about_presence() {
    // No remote, so no liveness signal — and "no signal" must stay "no opinion"
    // rather than becoming "everyone is offline", which would empty the roster.
    let network = LocalBridgeNetwork::new();
    let hub = network.bind("lonely-hub").unwrap();
    let bridge = RoutingBridge::local_only(hub);
    assert!(bridge.presence(&["GRVaddr".to_string()]).await.is_empty());
}

#[tokio::test]
async fn a_device_local_address_is_answered_here_not_asked_of_the_relay() {
    // The regression this exists to catch: a host bound in this process is not
    // a host-link identity and never heartbeats, so asking the relay returns
    // "no record" — which reads as offline and withholds every host on this
    // machine from the roster. Its liveness is knowable exactly right here.
    let network = LocalBridgeNetwork::new();
    let hub = network.bind("presence-hub").unwrap();
    let _host = network.bind("this-device").unwrap();
    // The remote answers everything as down, so anything forwarded to it is
    // visible as `false` in the result.
    let remote = Arc::new(RecordingRemote::default());
    let bridge = RoutingBridge::new(hub, remote as Arc<dyn Bridge>);

    let answered = bridge
        .presence(&["this-device".to_string(), "GRVremote".to_string()])
        .await;

    assert_eq!(
        answered.get("this-device"),
        Some(&true),
        "a bound local host is up, and the relay is never consulted about it"
    );
    assert_eq!(
        answered.get("GRVremote"),
        Some(&false),
        "a remote address still goes to the relay"
    );
}
