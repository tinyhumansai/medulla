//! Contract tests for bridge selection and device-local message delivery.

use super::{Bridge, BridgeKind, BridgeTransport, LocalBridgeNetwork};

#[test]
fn bridge_kind_parses_only_the_two_supported_scopes() {
    assert_eq!(BridgeKind::parse("local"), Some(BridgeKind::Local));
    assert_eq!(BridgeKind::parse("  Link  "), Some(BridgeKind::Link));
    assert_eq!(BridgeKind::parse("remote"), None);
    assert_eq!(BridgeKind::parse("tinyplace"), None);
    assert_eq!(BridgeKind::Local.as_str(), "local");
    assert_eq!(BridgeKind::Link.as_str(), "link");
}

#[tokio::test]
async fn local_messages_stay_on_the_shared_network_and_preserve_sender() {
    let network = LocalBridgeNetwork::new();
    let owner = network.bind("owner").unwrap();
    let worker = network.bind("worker").unwrap();

    owner.send("worker", "task").await.unwrap();
    assert!(owner.drain_inbox(10).await.is_empty());
    assert_eq!(
        worker
            .drain_inbox(10)
            .await
            .into_iter()
            .map(|message| (message.from, message.text))
            .collect::<Vec<_>>(),
        vec![("owner".to_string(), "task".to_string())]
    );
}

#[tokio::test]
async fn local_delivery_is_bounded_destructive_and_isolated() {
    let first = LocalBridgeNetwork::new();
    let sender = first.bind("sender").unwrap();
    let receiver = first.bind("receiver").unwrap();
    sender.send("receiver", "one").await.unwrap();
    sender.send("receiver", "two").await.unwrap();

    assert_eq!(receiver.drain_inbox(1).await[0].text, "one");
    assert_eq!(receiver.drain_inbox(10).await[0].text, "two");
    assert!(receiver.drain_inbox(10).await.is_empty());

    let second = LocalBridgeNetwork::new();
    let isolated = second.bind("sender").unwrap();
    assert!(isolated.send("receiver", "escape").await.is_err());
}

#[tokio::test]
async fn local_contacts_are_endpoint_discovery_without_remote_side_effects() {
    let network = LocalBridgeNetwork::new();
    let owner = network.bind("@owner").unwrap();
    assert!(!owner.contact_accepted("worker").await);

    let worker = network.bind("worker").unwrap();
    assert!(owner.request_contact("worker").await.is_ok());
    assert!(owner.contact_accepted("worker").await);
    assert_eq!(
        worker.resolve_handle("owner").await.as_deref(),
        Some("@owner")
    );
}

#[tokio::test]
async fn runtime_selected_local_bridge_uses_the_common_contract() {
    let network = LocalBridgeNetwork::new();
    let owner = BridgeTransport::Local(network.bind("owner").unwrap());
    let worker = BridgeTransport::Local(network.bind("worker").unwrap());
    assert_eq!(owner.kind(), BridgeKind::Local);

    owner.send("worker", "hello").await.unwrap();
    assert_eq!(worker.drain_inbox(1).await[0].text, "hello");
}

#[test]
fn local_addresses_are_unique_until_the_last_endpoint_clone_drops() {
    let network = LocalBridgeNetwork::new();
    let endpoint = network.bind("worker").unwrap();
    let clone = endpoint.clone();
    assert!(network.bind("worker").is_err());
    drop(endpoint);
    assert!(network.bind("worker").is_err());
    drop(clone);
    assert!(network.bind("worker").is_ok());
}
