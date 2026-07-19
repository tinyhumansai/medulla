//! Boot + registration behaviors: the mock Signal server serves loopback, two
//! identities register and round-trip encrypted DMs (ciphertext-only on the
//! wire), and the presence/contacts REST surface the runtime loops drive is
//! served faithfully.

use std::sync::Arc;
use std::time::Duration;

use medulla::tinyplace::tinyplace::{Signer, TinyPlaceClient, TinyPlaceClientOptions};

use crate::helpers::*;
use crate::mock_signal_server::MockSignalServer;

#[tokio::test]
async fn mock_signal_server_boots_and_serves_loopback() {
    let server = MockSignalServer::start().await;
    assert!(server.base_url.starts_with("http://127.0.0.1:"));
    // A fresh server has an empty stored log and zero counters.
    assert_eq!(server.controls().queued_messages(), 0);
    assert_eq!(server.controls().sends(), 0);
    assert!(server.controls().stored_envelopes().is_empty());
}

// Scenario 1: two identities (owner + worker) register (publish bundles),
// exchange bundles, and round-trip encrypted DMs through the real SDK. The
// server-side payloads are asserted ciphertext (no plaintext leakage) and each
// decryption yields the exact sent frame.
#[tokio::test]
async fn two_identities_register_and_roundtrip_encrypted_dms() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner", &server.base_url);
    let worker = make_identity("worker", &server.base_url);

    // Registration: both publish signed + one-time pre-keys so either can open a
    // channel to the other.
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();

    let owner_id = owner.id();
    let worker_id = worker.id();

    const TO_WORKER: &str = "PLAINTEXT-owner-to-worker-marker";
    const TO_OWNER: &str = "PLAINTEXT-worker-to-owner-marker";

    // owner → worker (opens a session from worker's bundle: X3DH + first ratchet).
    owner.transport.send(&worker_id, TO_WORKER).await.unwrap();
    let inbox = worker.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].from, owner_id);
    assert_eq!(inbox[0].text, TO_WORKER, "decryption yields the sent frame");

    // worker → owner reply (worker now opens its own session to owner).
    worker.transport.send(&owner_id, TO_OWNER).await.unwrap();
    let inbox = owner.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].text, TO_OWNER);

    // A second owner → worker send reuses the established session (no new bundle).
    let fetches_before = server.controls().bundle_fetches();
    owner
        .transport
        .send(&worker_id, "PLAINTEXT-again-marker")
        .await
        .unwrap();
    assert_eq!(
        server.controls().bundle_fetches(),
        fetches_before,
        "an established session does not re-fetch a bundle"
    );
    let inbox = worker.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].text, "PLAINTEXT-again-marker");

    // The server only ever saw ciphertext: none of the plaintext markers leaked
    // into any stored envelope (raw JSON or base64-decoded body bytes).
    server.assert_ciphertext_only(&[
        TO_WORKER,
        TO_OWNER,
        "PLAINTEXT-again-marker",
        "owner-to-worker",
        "worker-to-owner",
    ]);

    // Acks drained every delivered envelope from the live queue.
    assert_eq!(server.controls().queued_messages(), 0);
    assert!(server.controls().acks() >= 3);
}

// Scenario 1 (cont.): the presence + contacts REST surface the runtime loops
// drive is served faithfully — heartbeat, batch presence query, and the
// contact-accept handshake — against the same mock Signal server.
#[tokio::test]
async fn presence_and_contacts_rest_surface() {
    use medulla::tinyplace::{spawn_contact_auto_accepter, spawn_presence_heartbeat};

    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-rest", &server.base_url);
    let worker = make_identity("worker-rest", &server.base_url);
    let owner_id = owner.id();
    let worker_id = worker.id();

    // The worker reports the owner online.
    server
        .controls()
        .set_online(std::slice::from_ref(&owner_id));
    // A pending contact request from the owner, which the worker auto-accepts.
    server.controls().add_pending_contact(&owner_id);

    // Rebuild a bare client for the worker (the transport does not expose one).
    let worker_client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url: server.base_url.clone(),
        signer: Some(worker.signer.clone() as Arc<dyn Signer>),
        ..Default::default()
    });

    let heartbeat = spawn_presence_heartbeat(worker_client.clone(), Duration::from_millis(50));
    let allowed = owner_id.clone();
    let accepter = spawn_contact_auto_accepter(
        worker_client.clone(),
        Duration::from_millis(50),
        move |id: &str| id == allowed,
    );

    // Heartbeats flow and the contact is accepted.
    assert!(
        poll_until(T, || server.controls().heartbeats() >= 1).await,
        "presence heartbeat never reached the server"
    );
    assert!(
        poll_until(T, || server.controls().accepted().contains(&owner_id)).await,
        "contact auto-accept never reached the server"
    );

    heartbeat.abort();
    accepter.abort();

    // A direct presence query resolves the seeded online/offline split.
    let response = worker_client
        .presence
        .query(&[owner_id.clone(), worker_id.clone()])
        .await
        .unwrap();
    let online: std::collections::HashMap<String, bool> = response
        .presence
        .into_iter()
        .map(|p| (p.crypto_id, p.online))
        .collect();
    assert_eq!(online.get(&owner_id), Some(&true));
    assert_eq!(online.get(&worker_id), Some(&false));
    assert!(server.controls().presence_queries() >= 1);
}
