//! Encrypted Signal DM transport for the daemon.
//!
//! The daemon speaks to peers over tiny.place's Signal end-to-end encryption: the
//! same X3DH + double-ratchet primitives the SDK ships, persisted through the
//! `tinyplace-proto` [`FileSessionStore`]. There is no higher-level "encrypted
//! messaging" facade in the Rust SDK, so this module wires the pieces the SDK CLI
//! and examples use: [`SignalSession::encrypt`]/`decrypt` over a
//! [`FileSessionStore`], the REST key-bundle/pre-key endpoints, and the message
//! relay.
//!
//! Every wallet operation that touches the ratchet (encrypt-send, decrypt-read)
//! is serialized through one async lock: overlapping ratchet advances on a single
//! wallet corrupt session state and silently drop messages. Contact-accept and
//! presence are pure REST (no ratchet) and run unlocked.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use tokio::sync::Mutex;

use crate::tinyplace::FileSessionStore;
use ::tinyplace::crypto::decode_base58;
use ::tinyplace::signal::crypto::{ed25519_pub_to_x25519_pub, ed25519_seed_to_x25519_keypair};
use ::tinyplace::signal::session::SignalSession;
use ::tinyplace::signal::store::SessionStore;
use ::tinyplace::types::MessageEnvelope;
use ::tinyplace::{LocalSigner, Signer, TinyPlaceClient};

/// Render a tiny.place SDK error for a log line, keeping the server's response
/// body when there is one.
///
/// [`tinyplace::Error`]'s `Display` renders an HTTP failure as `HTTP <status>:
/// <path>` and drops [`HttpError::body`] entirely, so an operator sees *that* a
/// request was rejected but never *why*. The body is the only place the backend
/// explains itself (`{"error":"signature is required"}`), which is exactly what
/// is needed to tell a stale client apart from a moved server.
pub(super) fn describe_error(err: &::tinyplace::Error) -> String {
    match err {
        ::tinyplace::Error::Http(http) => {
            let body = http.body.to_string();
            // A body-less error renders as JSON `null`; don't append noise.
            if body == "null" || body.is_empty() {
                err.to_string()
            } else {
                format!("{err} — {body}")
            }
        }
        other => other.to_string(),
    }
}

/// One decrypted inbound DM.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub from: String,
    pub text: String,
}

/// Mint a directory-unique message id.
///
/// The directory rejects an envelope with an empty `id` (`400 message id, from,
/// and to are required`) and the Rust SDK's `messages.send` only defaults the
/// `timestamp`, so the id has to be supplied here. Matches the reference
/// TypeScript SDK's `msg_<millis>_<counter>` shape; the counter disambiguates
/// envelopes minted within the same millisecond.
fn next_message_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
    format!("msg_{millis}_{n}")
}

/// Encrypted transport bound to one machine wallet.
#[derive(Clone)]
pub struct SignalTransport {
    client: TinyPlaceClient,
    session: Arc<SignalSession>,
    store: Arc<FileSessionStore>,
    our_agent_id: String,
    our_ed25519_pub: [u8; 32],
    /// Serializes ratchet-touching ops (encrypt/decrypt) on this wallet.
    lock: Arc<Mutex<()>>,
}

impl SignalTransport {
    /// Build a transport for `signer`, persisting Signal state under
    /// `identity_dir/signal/<agent>.json`.
    pub fn new(
        client: TinyPlaceClient,
        signer: &LocalSigner,
        identity_dir: &std::path::Path,
    ) -> Self {
        let seed = signer.seed();
        let identity_key_pair = ed25519_seed_to_x25519_keypair(&seed);
        let our_x25519_pub = identity_key_pair.public_key;
        let our_agent_id = signer.agent_id();
        let path = FileSessionStore::default_path(identity_dir, &our_agent_id);
        let store = Arc::new(FileSessionStore::new(path, identity_key_pair));
        let session = Arc::new(SignalSession::new(
            store.clone() as Arc<dyn SessionStore>,
            our_x25519_pub,
        ));
        SignalTransport {
            client,
            session,
            store,
            our_agent_id,
            our_ed25519_pub: *signer.public_key(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// This wallet's agent id.
    pub fn agent_id(&self) -> &str {
        &self.our_agent_id
    }

    /// Drop the local Signal session with `peer` so the next send re-runs X3DH
    /// (a fresh `PREKEY_BUNDLE`). Used by senders to recover from a one-sided
    /// session — e.g. after the peer restarted and lost its ratchet state, our
    /// `CIPHERTEXT` becomes undecryptable and is silently dropped.
    ///
    /// Best-effort: the following resend re-runs X3DH regardless, so a store
    /// failure here surfaces as that resend's error rather than being fatal. We
    /// log it so a persistently-unwritable store is visible instead of looking
    /// like a clean recovery that keeps failing.
    pub async fn reset_session(&self, peer: &str) {
        let _guard = self.lock.lock().await;
        if let Err(e) = self.session.remove_session(peer).await {
            eprintln!("medulla: reset_session for {peer} failed to clear the store ({e}) — resend will retry X3DH anyway");
        }
    }

    /// Whether `peer` has *accepted* our contact request.
    ///
    /// A request only creates a `pending` edge; the peer's auto-accepter settles
    /// it a poll later. Since the relay refuses a DM between non-contacts
    /// (`403 not_a_contact`), callers wait on this before the first send rather
    /// than racing into a 403. Any lookup failure reads as "not accepted".
    pub async fn contact_accepted(&self, peer: &str) -> bool {
        self.client
            .contacts
            .status(peer)
            .await
            .map(|c| c.status == "accepted")
            .unwrap_or(false)
    }

    /// Ask `peer` for a contact relationship.
    ///
    /// The directory refuses a direct message between agents that are not
    /// contacts (`403 not_a_contact`), so a sender must request one before its
    /// first DM. Requesting an existing contact is harmless, which keeps this
    /// safe to call on every start.
    pub async fn request_contact(&self, peer: &str) -> Result<(), String> {
        self.client
            .contacts
            .request(peer)
            .await
            .map(|_| ())
            .map_err(|e| describe_error(&e))
    }

    /// This wallet's Ed25519 identity public key, base64 — the value the
    /// directory stores as `identityKey`.
    ///
    /// It must be the Ed25519 key, not the X25519 one derived from the same
    /// seed: the server validates a published pre-key by verifying its Ed25519
    /// `signature` against this field (`validSignedKeyForIdentity`). Sending the
    /// X25519 form makes every publish fail with `400 invalid input`, because an
    /// X25519 point cannot verify an Ed25519 signature.
    pub fn identity_key_base64(&self) -> String {
        BASE64.encode(self.our_ed25519_pub)
    }

    /// Ensure this wallet has a usable, relay-consistent key bundle,
    /// **idempotently**. Safe to call on every boot and periodically.
    ///
    /// Delegates to the tiny.place SDK's
    /// [`maintain_keys`](::tinyplace::signal::maintain::maintain_keys), which
    /// publishes only when the relay actually needs it: a no-op when the store
    /// already holds a signed pre-key and the relay's one-time pool is healthy, a
    /// (re)publish on first boot, a wiped store, or a low/depleted pool. Not
    /// republishing on a healthy restart is what prevents orphaning the keys the
    /// relay still serves.
    pub async fn publish_keys(&self, signer: &LocalSigner) -> Result<(), String> {
        ::tinyplace::signal::maintain::maintain_keys(
            &self.client.keys,
            &*self.store,
            signer,
            &self.our_agent_id,
            &self.identity_key_base64(),
            &::tinyplace::signal::maintain::MaintainPolicy::default(),
        )
        .await
        .map(|_| ())
        .map_err(|e| describe_error(&e))
    }

    /// Encrypt and send `body` to `to`. On a Signal session error (poisoned
    /// ratchet) the session is dropped and the send retried once from a fresh
    /// X3DH bundle.
    pub async fn send(&self, to: &str, body: &str) -> Result<(), String> {
        let _guard = self.lock.lock().await;
        match self.encrypt_and_send(to, body).await {
            Ok(()) => Ok(()),
            Err(err) if is_session_error(&err) => {
                // Drop the desynced session so the retry re-runs X3DH. A store
                // failure here is logged (not fatal): the retry re-handshakes
                // regardless and surfaces its own error to the caller.
                if let Err(e) = self.session.remove_session(to).await {
                    eprintln!("medulla: failed to clear desynced session with {to} ({e}) — retrying send anyway");
                }
                self.encrypt_and_send(to, body).await
            }
            Err(err) => Err(err),
        }
    }

    async fn encrypt_and_send(&self, to: &str, body: &str) -> Result<(), String> {
        let peer_ed = decode_agent_id(to)?;
        let peer_x = ed25519_pub_to_x25519_pub(&peer_ed).map_err(|e| e.to_string())?;

        let has_session = self
            .session
            .has_session(to)
            .await
            .map_err(|e| e.to_string())?;
        let bundle = if has_session {
            None
        } else {
            Some(
                self.client
                    .keys
                    .get_bundle(to)
                    .await
                    .map_err(|e| describe_error(&e))?,
            )
        };

        let encrypted = self
            .session
            .encrypt(
                to,
                &peer_x,
                body.as_bytes(),
                bundle.as_ref(),
                Some(&peer_ed),
            )
            .await
            .map_err(|e| e.to_string())?;

        let envelope = MessageEnvelope {
            id: next_message_id(),
            from: self.our_agent_id.clone(),
            to: to.to_string(),
            timestamp: String::new(),
            // The directory rejects a zero device id (`400 deviceId must be
            // positive`). One wallet is one device here, so it is always 1 —
            // the same value the reference TypeScript SDK and the spec use.
            device_id: 1,
            envelope_type: encrypted.message_type,
            body: encrypted.body,
            content_hint: None,
            signal: Some(encrypted.signal),
        };
        self.client
            .messages
            .send(envelope)
            .await
            .map(|_| ())
            .map_err(|e| describe_error(&e))
    }

    /// Destructively read the inbox (up to `limit`): decrypt each message, hand
    /// back the plaintext, and acknowledge (delete) every delivered message so
    /// the relay does not redeliver it.
    pub async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage> {
        let _guard = self.lock.lock().await;
        let response = match self
            .client
            .messages
            .list(&self.our_agent_id, Some(limit))
            .await
        {
            Ok(response) => response,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::new();
        for message in response.messages {
            match self.decrypt(&message).await {
                Ok(text) => out.push(InboundMessage {
                    from: message.from.clone(),
                    text,
                }),
                // A dropped message is otherwise invisible — the daemon would
                // just never run a task it was sent. Log the reason (stale
                // ratchet, prekey mismatch, bad envelope) instead of swallowing.
                // Then drop our half-session with the sender so its re-handshake
                // (a fresh `PREKEY_BUNDLE`) establishes cleanly rather than
                // failing forever against poisoned ratchet state.
                Err(e) => {
                    eprintln!(
                        "medulla daemon: dropped undecryptable {:?} from {} ({e}) — resetting session",
                        message.envelope_type, message.from
                    );
                    if is_session_error(&e) {
                        // Best-effort: a failed clear leaves the poisoned session
                        // in place, so log it rather than pretend recovery
                        // succeeded. The sender's re-handshake produces a fresh
                        // envelope; this one is unrecoverable either way.
                        if let Err(re) = self.session.remove_session(&message.from).await {
                            eprintln!(
                                "medulla daemon: failed to reset session with {} ({re}) — future messages may keep failing until the store recovers",
                                message.from
                            );
                        }
                    }
                }
            }
            // Ack regardless: the ciphertext cannot be decrypted under the current
            // (absent/poisoned) session and never will be, so leaving it un-acked
            // would only cause the relay to redeliver the same undecryptable
            // envelope forever. Recovery comes from the sender's next envelope.
            let _ = self
                .client
                .messages
                .acknowledge(&message.id, &self.our_agent_id)
                .await;
        }
        out
    }

    async fn decrypt(&self, envelope: &MessageEnvelope) -> Result<String, String> {
        let sender_ed = decode_agent_id(&envelope.from)?;
        let sender_x = ed25519_pub_to_x25519_pub(&sender_ed).map_err(|e| e.to_string())?;
        let plaintext = self
            .session
            .decrypt(&envelope.from, &sender_x, envelope)
            .await
            .map_err(|e| format!("decrypt: {e}"))?;
        String::from_utf8(plaintext).map_err(|e| format!("utf8: {e}"))
    }
}

/// A base58 Solana agent id decodes to a 32-byte Ed25519 public key.
fn decode_agent_id(agent_id: &str) -> Result<[u8; 32], String> {
    let bytes = decode_base58(agent_id).map_err(|e| format!("invalid agent id: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "agent id does not decode to a 32-byte key (got {})",
            bytes.len()
        ));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// A send failure that dropping the Signal session can fix: a stale-ratchet /
/// decrypt / prekey / session fault. A not-yet-contact rejection or a bare HTTP
/// status is deliberately NOT matched.
pub fn is_session_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("ratchet")
        || lower.contains("decrypt")
        || lower.contains("prekey")
        || lower.contains("pre-key")
        || lower.contains("signal session")
        || lower.contains("no session")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_error_classifier() {
        assert!(is_session_error("MAC verification failed during decrypt"));
        assert!(is_session_error("No session for ABC"));
        assert!(is_session_error("Signed pre-key spk_1 not found"));
        assert!(!is_session_error("not_a_contact"));
        assert!(!is_session_error("HTTP 500"));
    }

    #[test]
    fn decode_agent_id_rejects_non_32_bytes() {
        assert!(decode_agent_id("!!!not-base58!!!").is_err());
    }

    #[test]
    fn decode_agent_id_accepts_a_real_32_byte_key() {
        // A freshly generated signer's agent id is a base58 32-byte Ed25519 key.
        let signer = LocalSigner::generate();
        let decoded = decode_agent_id(&signer.agent_id()).expect("valid agent id");
        assert_eq!(decoded.len(), 32);
    }

    #[test]
    fn session_error_classifier_matches_prekey_variants() {
        assert!(is_session_error("Key bundle rejected: bad signed pre-key"));
        assert!(is_session_error("prekey missing"));
        assert!(is_session_error("no session established"));
    }
}
