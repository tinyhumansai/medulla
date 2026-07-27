//! Live verification of the relay's push channel, against a real backend.
//!
//! Every other test in this repo is offline and deterministic, and these are
//! neither — so they are `#[ignore]`d and never run as part of `cargo test`.
//! Run them deliberately, against the endpoint the local identity is configured
//! for:
//!
//! ```sh
//! cargo test -p medulla --test live_inbox_push -- --ignored --nocapture
//! ```
//!
//! They need an onboarded tiny.place identity in `~/.tinyplace/config.json`
//! (`TINYPLACE_API_URL` selects the relay). `/a2a/{id}/stream` is owner-gated,
//! so a freshly generated key is not enough — the identity must own its
//! directory card.
//!
//! A note on what is *not* here. Routing a Signal-encrypted body to self does
//! not work, so these probes send plain envelopes through `client.messages`
//! rather than `SignalTransport::send`. One transport holds one session record
//! per peer, and when the peer is self the sending and receiving ratchets
//! collide on that record: a second send advances it past what the first
//! message needs, and the read fails with `MAC verification failed`. The
//! transport's own recovery clears the poisoned session, so nothing is left
//! broken — but a self-addressed encrypted round trip cannot be made to pass,
//! and proving one needs a second identity.
//!
//! What is actually being proven here, because none of it is provable offline:
//! that the upgrade authenticates at all, that the endpoint exists and is
//! reachable, and that a message relayed over HTTPS while the socket is open
//! comes back down it. The unit tests cover what the doorbell does with a ring;
//! only this covers whether a ring ever arrives.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use medulla::daemon::transport::SignalTransport;
use medulla::tinyplace::{load_or_create_identity, resolve_endpoint};
use tinyplace::types::MessageEnvelope;
use tinyplace::{Signer, TinyPlaceClient, TinyPlaceClientOptions};

/// How long to allow for a socket to open and a frame to land. Generous: this
/// crosses the public internet, and a slow answer is not the failure under test.
const PATIENCE: Duration = Duration::from_secs(20);

/// The configured identity plus a client for its relay.
///
/// Skips (rather than fails) when there is no local identity: a machine that has
/// never onboarded has nothing to verify, and failing there would just be noise.
fn live_client() -> Option<(TinyPlaceClient, Arc<tinyplace::LocalSigner>, String)> {
    let env: HashMap<String, String> = std::env::vars().collect();
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let config_file = medulla::tinyplace::config_path(&env, &home);
    if !config_file.exists() {
        eprintln!(
            "skipping: no tiny.place identity at {}",
            config_file.display()
        );
        return None;
    }
    let (signer, config) = load_or_create_identity(&config_file, &env).ok()?;
    let base_url = resolve_endpoint(&env, &config);
    let signer = Arc::new(signer);
    let client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url: base_url.clone(),
        signer: Some(signer.clone() as Arc<dyn Signer>),
        ..Default::default()
    });
    eprintln!("relay: {base_url}");
    eprintln!("agent: {}", signer.agent_id());
    Some((client, signer, base_url))
}

/// Put a plain note-to-self in the mailbox.
///
/// The relay exempts a self-addressed message from the contact gate, so this
/// needs no second identity and no accepted edge. The body is not encrypted —
/// nothing here decrypts it, and what is under test is whether the *relay*
/// pushes an envelope, not what the envelope contains.
async fn send_note_to_self(client: &TinyPlaceClient, agent_id: &str, marker: &str) {
    let envelope = MessageEnvelope {
        id: format!("live-push-{marker}"),
        from: agent_id.to_string(),
        to: agent_id.to_string(),
        timestamp: String::new(),
        device_id: 1,
        // The relay validates this against a fixed set; an empty string is
        // rejected with `invalid envelope type`. Nothing here decrypts the
        // body, so the label is all that matters.
        envelope_type: "CIPHERTEXT".to_string(),
        // The relay decodes the body before storing it, so a plain string is
        // rejected with `body must be base64 ciphertext`.
        body: base64::engine::general_purpose::STANDARD.encode(format!("live push probe {marker}")),
        content_hint: None,
        signal: None,
    };
    client
        .messages
        .send(envelope)
        .await
        .expect("the relay should accept a note-to-self");
}

/// Delete everything currently in the mailbox, so a probe leaves nothing behind.
async fn drain_mailbox(client: &TinyPlaceClient, agent_id: &str) {
    if let Ok(response) = client.messages.list(agent_id, Some(50)).await {
        for message in response.messages {
            let _ = client.messages.acknowledge(&message.id, agent_id).await;
        }
    }
}

#[tokio::test]
#[ignore = "hits a live relay; run explicitly with --ignored"]
async fn the_stream_authenticates_and_pushes_a_relayed_envelope() {
    let Some((client, signer, _)) = live_client() else {
        return;
    };
    let agent_id = signer.agent_id();

    // Start clean, so the snapshot below is not a pile of old mail.
    drain_mailbox(&client, &agent_id).await;

    // 1. The upgrade authenticates. This is the step that was entirely broken
    //    before the SDK bump, and whose auth form I could not verify offline.
    let mut connection = tokio::time::timeout(PATIENCE, client.a2a.stream(&agent_id).connect())
        .await
        .expect("the upgrade should not hang")
        .expect("the upgrade should authenticate and connect");
    eprintln!("✓ upgrade authenticated");

    // 2. The relay opens with a snapshot frame. Its arrival is what the doorbell
    //    reacts to on connect.
    let snapshot = tokio::time::timeout(PATIENCE, connection.recv())
        .await
        .expect("a snapshot should arrive")
        .expect("the stream should not close first")
        .expect("the snapshot should be valid json");
    eprintln!("✓ snapshot frame: type={:?}", snapshot.get("type"));
    assert!(
        snapshot.get("type").and_then(|t| t.as_str()).is_some(),
        "every frame carries a string `type`: {snapshot}"
    );

    // 3. The live tail. A message relayed over HTTPS *while the socket is open*
    //    must come back down it — this is the whole premise, and the only part
    //    that cannot be inferred from the snapshot.
    send_note_to_self(&client, &agent_id, "tail").await;
    let pushed = tokio::time::timeout(PATIENCE, connection.recv())
        .await
        .expect("a relayed envelope should be pushed within the timeout")
        .expect("the stream should still be open")
        .expect("the pushed frame should be valid json");
    eprintln!("✓ live tail frame: type={:?}", pushed.get("type"));

    let _ = connection.close().await;
    drain_mailbox(&client, &agent_id).await;
}

#[tokio::test]
#[ignore = "hits a live relay; run explicitly with --ignored"]
async fn the_listener_rings_the_doorbell_for_a_relayed_message() {
    // The same thing one layer up: not "does a frame arrive" but "does medulla's
    // mailbox loop stop waiting because of it".
    let Some((client, signer, _)) = live_client() else {
        return;
    };
    let agent_id = signer.agent_id();
    let identity_dir = medulla::tinyplace::config_path(
        &std::env::vars().collect::<HashMap<_, _>>(),
        &dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
    )
    .parent()
    .map(PathBuf::from)
    .expect("the config file has a parent directory");

    drain_mailbox(&client, &agent_id).await;

    let transport = SignalTransport::new(client.clone(), &signer, &identity_dir);
    let _listener = transport.spawn_inbox_listener(Some(Arc::new(|line: &str| {
        eprintln!("  listener: {line}");
    })));

    // The socket has to come up before any of this means anything.
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !transport.is_push_listening() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        transport.is_push_listening(),
        "the push channel should be open within {PATIENCE:?}"
    );
    eprintln!("✓ push channel reports listening");

    // Consume the ring the connect snapshot already caused, so what is measured
    // below is the *new* message and not the one that was pending.
    transport.wait_for_inbox(Duration::from_millis(200)).await;

    // With a poll interval of ten minutes, returning at all can only be the
    // doorbell — there is no timeout that could have fired.
    //
    // Timed from *before* the send, so the figure is the honest end-to-end
    // "relayed → the loop knows". Measuring after the send completes reports
    // microseconds, because the push routinely beats the HTTPS response and the
    // ring is already pending by then.
    let started = std::time::Instant::now();
    send_note_to_self(&client, &agent_id, "doorbell").await;
    tokio::time::timeout(PATIENCE, transport.wait_for_inbox(Duration::from_secs(600)))
        .await
        .expect("the doorbell should ring long before a ten-minute poll");
    eprintln!(
        "✓ doorbell rang {:?} after the send began (poll was 600s)",
        started.elapsed()
    );

    drain_mailbox(&client, &agent_id).await;
}
