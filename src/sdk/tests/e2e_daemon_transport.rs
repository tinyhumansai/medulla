//! End-to-end coverage for the daemon's encrypted Signal DM transport
//! ([`medulla::daemon::transport::SignalTransport`]) with no network: two local
//! identities in tempdirs exchange key bundles and messages through an in-memory
//! [`MockRelay`], driving the X3DH + double-ratchet encrypt/decrypt path, the
//! session self-heal retry, and the list/decrypt error branches.

mod support;

#[path = "support/mock_harness_relay.rs"]
mod mock_harness_relay;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla::daemon::transport::{is_session_error, SignalTransport};
use medulla::tinyplace_support::tinyplace::{
    LocalSigner, Signer, TinyPlaceClient, TinyPlaceClientOptions,
};

use mock_harness_relay::MockRelay;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temp dir removed on drop, hosting one identity's Signal state.
struct IdentityDir {
    path: PathBuf,
}

impl IdentityDir {
    fn new(tag: &str) -> Self {
        let id = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "medulla-transport-{tag}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        IdentityDir { path }
    }
}

impl Drop for IdentityDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A fully wired identity: signer, a relay-pointed client, its transport, and the
/// temp dir keeping its on-disk Signal state alive.
struct Identity {
    transport: SignalTransport,
    signer: Arc<LocalSigner>,
    _dir: IdentityDir,
}

fn make_identity(tag: &str, base_url: &str) -> Identity {
    let signer = Arc::new(LocalSigner::generate());
    let client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url: base_url.to_string(),
        signer: Some(signer.clone() as Arc<dyn Signer>),
        ..Default::default()
    });
    let dir = IdentityDir::new(tag);
    let transport = SignalTransport::new(client, &signer, &dir.path);
    Identity {
        transport,
        signer,
        _dir: dir,
    }
}

#[tokio::test]
async fn publish_keys_and_encrypted_roundtrip() {
    let relay = MockRelay::start().await;
    let alice = make_identity("alice", &relay.base_url);
    let bob = make_identity("bob", &relay.base_url);

    // Both sides publish pre-keys so either can open a channel to the other.
    alice.transport.publish_keys(&alice.signer).await.unwrap();
    bob.transport.publish_keys(&bob.signer).await.unwrap();

    let bob_id = bob.signer.agent_id();
    let alice_id = alice.signer.agent_id();

    // Trivial getters.
    assert_eq!(alice.transport.agent_id(), alice_id);
    assert!(!alice.transport.identity_key_base64().is_empty());

    // alice → bob (opens a session from bob's bundle).
    alice.transport.send(&bob_id, "hello bob").await.unwrap();
    let inbox = bob.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].from, alice_id);
    assert_eq!(inbox[0].text, "hello bob");

    // The relay dropped the delivered message on ack.
    assert_eq!(relay.controls().queued_messages(), 0);

    // bob → alice reply (bob now opens its own session to alice).
    bob.transport.send(&alice_id, "hi alice").await.unwrap();
    let inbox = alice.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].text, "hi alice");

    // A second alice → bob send reuses the established session (no new bundle).
    alice.transport.send(&bob_id, "again").await.unwrap();
    let inbox = bob.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].text, "again");
}

#[tokio::test]
async fn self_heals_when_first_bundle_is_rejected() {
    let relay = MockRelay::start().await;
    let alice = make_identity("alice", &relay.base_url);
    let bob = make_identity("bob", &relay.base_url);
    alice.transport.publish_keys(&alice.signer).await.unwrap();
    bob.transport.publish_keys(&bob.signer).await.unwrap();

    // Arm a corrupt signed-pre-key signature on the next bundle: the first
    // encrypt rejects it (a session-shaped error), then the retry re-fetches a
    // valid bundle and succeeds.
    relay.controls().corrupt_next_bundle();
    let bob_id = bob.signer.agent_id();
    alice.transport.send(&bob_id, "resilient").await.unwrap();

    let inbox = bob.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].text, "resilient");
}

#[tokio::test]
async fn send_without_published_bundle_errors() {
    let relay = MockRelay::start().await;
    let alice = make_identity("alice", &relay.base_url);
    let bob = make_identity("bob", &relay.base_url);
    alice.transport.publish_keys(&alice.signer).await.unwrap();
    // bob never publishes → no bundle → get_bundle 404 → non-session error.

    let bob_id = bob.signer.agent_id();
    let err = alice
        .transport
        .send(&bob_id, "anyone home?")
        .await
        .unwrap_err();
    assert!(!is_session_error(&err), "404 is not a self-healable error");
}

#[tokio::test]
async fn send_to_invalid_agent_id_errors() {
    let relay = MockRelay::start().await;
    let alice = make_identity("alice", &relay.base_url);
    alice.transport.publish_keys(&alice.signer).await.unwrap();

    let err = alice
        .transport
        .send("!!!not-base58!!!", "hi")
        .await
        .unwrap_err();
    assert!(err.contains("invalid agent id"), "got: {err}");
}

#[tokio::test]
async fn drain_inbox_tolerates_list_failure() {
    let relay = MockRelay::start().await;
    let bob = make_identity("bob", &relay.base_url);
    bob.transport.publish_keys(&bob.signer).await.unwrap();

    relay.controls().fail_list(true);
    let inbox = bob.transport.drain_inbox(50).await;
    assert!(inbox.is_empty(), "a list failure yields an empty drain");
}

#[tokio::test]
async fn drain_inbox_skips_undecryptable_message_but_acks_it() {
    let relay = MockRelay::start().await;
    let alice = make_identity("alice", &relay.base_url);
    let bob = make_identity("bob", &relay.base_url);
    bob.transport.publish_keys(&bob.signer).await.unwrap();

    // A message from a valid agent id but with garbage ciphertext: decrypt fails,
    // so it is skipped — yet still acknowledged (removed) so it never redelivers.
    relay.controls().inject_message(
        &alice.signer.agent_id(),
        &bob.signer.agent_id(),
        "not-cipher",
    );
    let inbox = bob.transport.drain_inbox(50).await;
    assert!(inbox.is_empty(), "undecryptable message is dropped");
    assert_eq!(
        relay.controls().queued_messages(),
        0,
        "the message was still acked"
    );
}
