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
use ::tinyplace::signal::keys::{generate_pre_keys, generate_signed_pre_key, serialize_pre_key};
use ::tinyplace::signal::session::SignalSession;
use ::tinyplace::signal::store::SessionStore;
use ::tinyplace::types::{MessageEnvelope, PreKeysRequest, SignedPreKeyRequest};
use ::tinyplace::{LocalSigner, Signer, TinyPlaceClient};

/// How many one-time pre-keys to publish on onboard.
const ONE_TIME_PRE_KEY_COUNT: usize = 20;

/// Render a tiny.place SDK error for a log line, keeping the server's response
/// body when there is one.
///
/// [`tinyplace::Error`]'s `Display` renders an HTTP failure as `HTTP <status>:
/// <path>` and drops [`HttpError::body`] entirely, so an operator sees *that* a
/// request was rejected but never *why*. The body is the only place the backend
/// explains itself (`{"error":"signature is required"}`), which is exactly what
/// is needed to tell a stale client apart from a moved server.
fn describe_error(err: &::tinyplace::Error) -> String {
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
            .map_err(|e| e.to_string())
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

    /// Generate and publish a signed pre-key + one-time pre-keys so peers can run
    /// X3DH against this wallet. Idempotent enough to run on every start; stores
    /// the private material locally and uploads the public parts.
    pub async fn publish_keys(&self, signer: &LocalSigner) -> Result<(), String> {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let spk_id = format!("spk_{now_secs}");
        let spk = generate_signed_pre_key(signer, &spk_id)
            .await
            .map_err(|e| e.to_string())?;
        self.store
            .store_signed_pre_key(spk.clone())
            .await
            .map_err(|e| e.to_string())?;
        let one_time = generate_pre_keys(signer, now_secs, ONE_TIME_PRE_KEY_COUNT)
            .await
            .map_err(|e| e.to_string())?;
        for key in &one_time {
            self.store
                .store_pre_key(key.clone())
                .await
                .map_err(|e| e.to_string())?;
        }

        let identity_key = self.identity_key_base64();
        self.client
            .keys
            .rotate_signed_pre_key(
                &self.our_agent_id,
                &SignedPreKeyRequest {
                    identity_key: Some(identity_key.clone()),
                    signed_pre_key: serialize_pre_key(&spk),
                },
            )
            .await
            .map_err(|e| describe_error(&e))?;
        self.client
            .keys
            .upload_pre_keys(
                &self.our_agent_id,
                &PreKeysRequest {
                    identity_key: Some(identity_key),
                    pre_keys: one_time.iter().map(serialize_pre_key).collect(),
                },
            )
            .await
            .map_err(|e| describe_error(&e))?;
        Ok(())
    }

    /// Encrypt and send `body` to `to`. On a Signal session error (poisoned
    /// ratchet) the session is dropped and the send retried once from a fresh
    /// X3DH bundle.
    pub async fn send(&self, to: &str, body: &str) -> Result<(), String> {
        let _guard = self.lock.lock().await;
        match self.encrypt_and_send(to, body).await {
            Ok(()) => Ok(()),
            Err(err) if is_session_error(&err) => {
                // Drop the desynced session so the retry re-runs X3DH.
                let _ = self.session.remove_session(to).await;
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
                    .map_err(|e| e.to_string())?,
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
            .map_err(|e| e.to_string())
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
            if let Some(text) = self.decrypt(&message).await {
                out.push(InboundMessage {
                    from: message.from.clone(),
                    text,
                });
            }
            let _ = self
                .client
                .messages
                .acknowledge(&message.id, &self.our_agent_id)
                .await;
        }
        out
    }

    async fn decrypt(&self, envelope: &MessageEnvelope) -> Option<String> {
        let sender_ed = decode_agent_id(&envelope.from).ok()?;
        let sender_x = ed25519_pub_to_x25519_pub(&sender_ed).ok()?;
        let plaintext = self
            .session
            .decrypt(&envelope.from, &sender_x, envelope)
            .await
            .ok()?;
        String::from_utf8(plaintext).ok()
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
