//! Agent-runtime helpers layered on the tinyplace SDK client.
//!
//! These are thin async orchestrations of SDK calls — no PTY/provider spawning,
//! no rendering. They give the TUI/daemon the pieces it needs to stay live on
//! tiny.place:
//!
//! - [`FileSessionStore`] — a filesystem [`SessionStore`] persisting Signal
//!   ratchet/pre-key state as JSON, laid out to coexist with the TS SDK's
//!   `FileSessionStore`.
//! - [`load_or_create_identity`] — load-or-mint a 32-byte Ed25519 seed via the
//!   SDK signer, persisted to the tinyplace CLI config file (`secretKey` hex).
//! - [`spawn_mailbox_poll`] — poll + destructively read DMs, decoding task
//!   frames, over a tokio channel.
//! - [`spawn_contact_auto_accepter`] — poll contact requests and accept via a
//!   fail-closed allowlist.
//! - [`spawn_presence_heartbeat`] — keep the identity marked online.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use tinyplace::error::{Error as SdkError, Result as SdkResult};
use tinyplace::signal::crypto::X25519KeyPair;
use tinyplace::signal::keys::{PreKeyPair, SignedPreKeyPair};
use tinyplace::signal::store::{SessionState, SessionStore};
use tinyplace::types::MessageEnvelope;
use tinyplace::{LocalSigner, TinyPlaceClient};

use super::config::{load_config, write_config, TinyPlaceConfig};
use super::frames::{decode_task_frame, TaskFrame};

/// Errors from the runtime helpers.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("tinyplace sdk error: {0}")]
    Sdk(#[from] SdkError),
    #[error("{0}")]
    Invalid(String),
}

/// Result alias for the runtime helpers.
pub type RuntimeResult<T> = Result<T, RuntimeError>;

impl From<RuntimeError> for SdkError {
    fn from(err: RuntimeError) -> Self {
        match err {
            RuntimeError::Sdk(sdk) => sdk,
            other => SdkError::InvalidArgument(other.to_string()),
        }
    }
}

// ─── identity bootstrap ─────────────────────────────────────────────────────

/// Load or create the agent identity.
///
/// A 32-byte Ed25519 seed is resolved from `TINYPLACE_SECRET_KEY` (hex) in `env`,
/// then the config file's `secretKey`. When neither is set, a fresh seed is
/// generated and persisted to `config_path` (atomic, `0600`). Returns the signer
/// and the (possibly updated) config. The config's `secret_key` always reflects
/// the seed in use.
pub fn load_or_create_identity(
    config_path: &Path,
    env: &HashMap<String, String>,
) -> RuntimeResult<(LocalSigner, TinyPlaceConfig)> {
    let mut config = load_config(config_path);

    let from_env = env
        .get("TINYPLACE_SECRET_KEY")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let existing = from_env.or_else(|| {
        config
            .secret_key
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    });

    if let Some(hex) = existing {
        let seed = decode_seed_hex(&hex)?;
        let signer = LocalSigner::from_seed(&seed)?;
        config.secret_key = Some(hex);
        return Ok((signer, config));
    }

    // No key anywhere: mint one and persist it.
    let signer = LocalSigner::generate();
    let hex = encode_seed_hex(&signer.seed());
    config.secret_key = Some(hex);
    write_config(config_path, &config)?;
    Ok((signer, config))
}

fn encode_seed_hex(seed: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in seed {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn decode_seed_hex(hex: &str) -> RuntimeResult<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(RuntimeError::Invalid(format!(
            "secret key must be 64 hex chars (a 32-byte seed), got {}",
            hex.len()
        )));
    }
    let mut seed = [0u8; 32];
    for (index, byte) in seed.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
            .map_err(|_| RuntimeError::Invalid("secret key is not valid hex".to_string()))?;
    }
    Ok(seed)
}

// ─── file session store ─────────────────────────────────────────────────────

// On-disk JSON shapes. Field names are camelCase and the layout mirrors the TS
// SDK's `FileSessionStore` (`version`, `signedPreKeys`, `activeSignedPreKeyId`,
// `preKeys`, `sessions`), so a file this store writes is readable by the TS CLI
// and vice-versa. Every byte array is base64; the skipped-key map is an array of
// `[id, base64]` pairs. NOTE: the Rust SDK's `SessionStore` trait has no group
// sender-key methods, so this store neither reads nor writes `senderKeysOwn` /
// `senderKeyReceivers`; rewriting a file that the TS group layer populated would
// drop those blocks. Group messaging is out of scope for the medulla runtime.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SerializedKeyPair {
    public_key: String,
    private_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SerializedPreKey {
    key_id: String,
    key_pair: SerializedKeyPair,
    signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SerializedSession {
    dh_send_key_pair: SerializedKeyPair,
    dh_recv_public_key: Option<String>,
    root_key: String,
    send_chain_key: Option<String>,
    recv_chain_key: Option<String>,
    send_message_number: u32,
    recv_message_number: u32,
    previous_chain_length: u32,
    skipped_keys: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistShape {
    #[serde(default = "one")]
    version: u8,
    #[serde(default)]
    signed_pre_keys: BTreeMap<String, SerializedPreKey>,
    #[serde(default)]
    active_signed_pre_key_id: Option<String>,
    #[serde(default)]
    pre_keys: BTreeMap<String, SerializedPreKey>,
    #[serde(default)]
    sessions: BTreeMap<String, SerializedSession>,
}

fn one() -> u8 {
    1
}

impl Default for PersistShape {
    fn default() -> Self {
        PersistShape {
            version: 1,
            signed_pre_keys: BTreeMap::new(),
            active_signed_pre_key_id: None,
            pre_keys: BTreeMap::new(),
            sessions: BTreeMap::new(),
        }
    }
}

/// A filesystem-backed [`SessionStore`]. All ratchet/pre-key state for one
/// identity lives in a single JSON file (`<dir>/signal/<address>.json`), written
/// atomically (temp file + rename) with `0600` permissions. The long-term
/// identity X25519 key pair is derived from the wallet seed and supplied at
/// construction — it is never written to disk.
///
/// This store keeps no in-memory cache: every operation reads the file fresh and
/// each mutation rewrites it atomically, so it stays coherent when another
/// process on the same wallet advances the ratchet. It does **not** lock: callers
/// sharing one wallet must serialize their operations (as the tinyplace machine
/// bus does).
pub struct FileSessionStore {
    path: PathBuf,
    identity_key_pair: X25519KeyPair,
}

impl FileSessionStore {
    /// Create a store persisting to `path`, bound to `identity_key_pair`.
    pub fn new(path: impl Into<PathBuf>, identity_key_pair: X25519KeyPair) -> Self {
        FileSessionStore {
            path: path.into(),
            identity_key_pair,
        }
    }

    /// The default per-identity path: `<dir>/signal/<sanitized-owner>.json`,
    /// mirroring the TS SDK's `FileSessionStore.defaultPath`. Non
    /// `[A-Za-z0-9_-]` characters in `owner_id` are replaced with `_` and the
    /// name is capped at 80 chars.
    pub fn default_path(dir: &Path, owner_id: &str) -> PathBuf {
        let mut safe: String = owner_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .take(80)
            .collect();
        if safe.is_empty() {
            safe = "default".to_string();
        }
        dir.join("signal").join(format!("{safe}.json"))
    }

    fn load(&self) -> RuntimeResult<PersistShape> {
        match std::fs::read_to_string(&self.path) {
            Ok(contents) => Ok(serde_json::from_str(&contents)?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(PersistShape::default()),
            Err(err) => Err(RuntimeError::Io(err)),
        }
    }

    fn flush(&self, state: &PersistShape) -> RuntimeResult<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(state)?;
        let pid = std::process::id();
        let tmp = self.path.with_extension(format!("json.tmp.{pid}"));
        std::fs::write(&tmp, json.as_bytes())?;
        set_owner_only(&tmp)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

// serde <-> signal-type conversions.

fn ser_key_pair(kp: &X25519KeyPair) -> SerializedKeyPair {
    SerializedKeyPair {
        public_key: BASE64.encode(kp.public_key),
        private_key: BASE64.encode(kp.private_key),
    }
}

fn de_key_pair(value: &SerializedKeyPair) -> RuntimeResult<X25519KeyPair> {
    Ok(X25519KeyPair {
        public_key: de_b32(&value.public_key)?,
        private_key: de_b32(&value.private_key)?,
    })
}

fn ser_pre_key(pre_key: &PreKeyPair) -> SerializedPreKey {
    SerializedPreKey {
        key_id: pre_key.key_id.clone(),
        key_pair: ser_key_pair(&pre_key.key_pair),
        signature: BASE64.encode(&pre_key.signature),
    }
}

fn de_pre_key(value: &SerializedPreKey) -> RuntimeResult<PreKeyPair> {
    Ok(PreKeyPair {
        key_id: value.key_id.clone(),
        key_pair: de_key_pair(&value.key_pair)?,
        signature: BASE64
            .decode(value.signature.as_bytes())
            .map_err(|err| RuntimeError::Invalid(format!("invalid base64 signature: {err}")))?,
    })
}

fn ser_session(session: &SessionState) -> SerializedSession {
    SerializedSession {
        dh_send_key_pair: ser_key_pair(&session.dh_send_key_pair),
        dh_recv_public_key: session.dh_recv_public_key.map(|k| BASE64.encode(k)),
        root_key: BASE64.encode(session.root_key),
        send_chain_key: session.send_chain_key.map(|k| BASE64.encode(k)),
        recv_chain_key: session.recv_chain_key.map(|k| BASE64.encode(k)),
        send_message_number: session.send_message_number,
        recv_message_number: session.recv_message_number,
        previous_chain_length: session.previous_chain_length,
        skipped_keys: session
            .skipped_keys
            .iter()
            .map(|(id, key)| (id.clone(), BASE64.encode(key)))
            .collect(),
    }
}

fn de_session(value: &SerializedSession) -> RuntimeResult<SessionState> {
    let mut skipped_keys = HashMap::new();
    for (id, encoded) in &value.skipped_keys {
        skipped_keys.insert(id.clone(), de_b32(encoded)?);
    }
    Ok(SessionState {
        dh_send_key_pair: de_key_pair(&value.dh_send_key_pair)?,
        dh_recv_public_key: value
            .dh_recv_public_key
            .as_deref()
            .map(de_b32)
            .transpose()?,
        root_key: de_b32(&value.root_key)?,
        send_chain_key: value.send_chain_key.as_deref().map(de_b32).transpose()?,
        recv_chain_key: value.recv_chain_key.as_deref().map(de_b32).transpose()?,
        send_message_number: value.send_message_number,
        recv_message_number: value.recv_message_number,
        previous_chain_length: value.previous_chain_length,
        skipped_keys,
    })
}

fn de_b32(value: &str) -> RuntimeResult<[u8; 32]> {
    let bytes = BASE64
        .decode(value.as_bytes())
        .map_err(|err| RuntimeError::Invalid(format!("invalid base64 key: {err}")))?;
    if bytes.len() != 32 {
        return Err(RuntimeError::Invalid(format!(
            "expected a 32-byte key, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[async_trait]
impl SessionStore for FileSessionStore {
    async fn identity_x25519_key_pair(&self) -> SdkResult<X25519KeyPair> {
        Ok(self.identity_key_pair.clone())
    }

    async fn signed_pre_key(&self, key_id: &str) -> SdkResult<Option<SignedPreKeyPair>> {
        let state = self.load()?;
        match state.signed_pre_keys.get(key_id) {
            Some(value) => Ok(Some(de_pre_key(value)?)),
            None => Ok(None),
        }
    }

    async fn active_signed_pre_key(&self) -> SdkResult<SignedPreKeyPair> {
        let state = self.load()?;
        let id = state
            .active_signed_pre_key_id
            .as_ref()
            .ok_or_else(|| SdkError::InvalidArgument("No active signed pre-key".into()))?;
        let value = state
            .signed_pre_keys
            .get(id)
            .ok_or_else(|| SdkError::InvalidArgument("Active signed pre-key not found".into()))?;
        Ok(de_pre_key(value)?)
    }

    async fn store_signed_pre_key(&self, pre_key: SignedPreKeyPair) -> SdkResult<()> {
        let mut state = self.load()?;
        let key_id = pre_key.key_id.clone();
        state
            .signed_pre_keys
            .insert(key_id.clone(), ser_pre_key(&pre_key));
        state.active_signed_pre_key_id = Some(key_id);
        self.flush(&state)?;
        Ok(())
    }

    async fn pre_key(&self, key_id: &str) -> SdkResult<Option<PreKeyPair>> {
        let state = self.load()?;
        match state.pre_keys.get(key_id) {
            Some(value) => Ok(Some(de_pre_key(value)?)),
            None => Ok(None),
        }
    }

    async fn remove_pre_key(&self, key_id: &str) -> SdkResult<()> {
        let mut state = self.load()?;
        state.pre_keys.remove(key_id);
        self.flush(&state)?;
        Ok(())
    }

    async fn store_pre_key(&self, pre_key: PreKeyPair) -> SdkResult<()> {
        let mut state = self.load()?;
        state
            .pre_keys
            .insert(pre_key.key_id.clone(), ser_pre_key(&pre_key));
        self.flush(&state)?;
        Ok(())
    }

    async fn all_pre_keys(&self) -> SdkResult<Vec<PreKeyPair>> {
        let state = self.load()?;
        let mut out = Vec::with_capacity(state.pre_keys.len());
        for value in state.pre_keys.values() {
            out.push(de_pre_key(value)?);
        }
        Ok(out)
    }

    async fn session(&self, address: &str) -> SdkResult<Option<SessionState>> {
        let state = self.load()?;
        match state.sessions.get(address) {
            Some(value) => Ok(Some(de_session(value)?)),
            None => Ok(None),
        }
    }

    async fn store_session(&self, address: &str, session: SessionState) -> SdkResult<()> {
        let mut state = self.load()?;
        state
            .sessions
            .insert(address.to_string(), ser_session(&session));
        self.flush(&state)?;
        Ok(())
    }

    async fn remove_session(&self, address: &str) -> SdkResult<()> {
        let mut state = self.load()?;
        state.sessions.remove(address);
        self.flush(&state)?;
        Ok(())
    }
}

// ─── mailbox poll loop ──────────────────────────────────────────────────────

/// One destructively-read inbound message, with the medulla task frame decoded
/// from it when the plaintext body is one.
#[derive(Debug, Clone)]
pub struct MailboxItem {
    /// The raw relay envelope (opaque `body` as delivered).
    pub envelope: MessageEnvelope,
    /// The plaintext body produced by `decode_body` (e.g. after Signal decrypt).
    pub body: String,
    /// The decoded `medulla-tinyplace/1` task frame, when `body` is one.
    pub frame: Option<TaskFrame>,
}

/// A running mailbox poll: the background task handle and the receiving end of
/// the decoded-message channel. Dropping `receiver` stops the loop after its
/// next send.
pub struct MailboxPoll {
    pub handle: JoinHandle<()>,
    pub receiver: mpsc::Receiver<MailboxItem>,
}

/// Spawn a loop that polls `client.messages` for `agent_id` every `interval`,
/// **destructively reads** each message (acknowledges/deletes it after handing it
/// off), turns the opaque body into plaintext via `decode_body` (the caller's
/// Signal-decrypt hook; return `None` to skip a message), decodes any task frame,
/// and yields [`MailboxItem`]s over a channel.
///
/// Best-effort: transient list/ack errors are ignored and retried next tick. The
/// loop ends when the receiver is dropped.
pub fn spawn_mailbox_poll<F>(
    client: TinyPlaceClient,
    agent_id: String,
    interval: Duration,
    limit: i64,
    decode_body: F,
) -> MailboxPoll
where
    F: Fn(&MessageEnvelope) -> Option<String> + Send + 'static,
{
    let (tx, receiver) = mpsc::channel(256);
    let handle = tokio::spawn(async move {
        loop {
            if let Ok(resp) = client.messages.list(&agent_id, Some(limit)).await {
                for msg in resp.messages {
                    if let Some(body) = decode_body(&msg) {
                        let frame = decode_task_frame(&body);
                        let item = MailboxItem {
                            envelope: msg.clone(),
                            body,
                            frame,
                        };
                        if tx.send(item).await.is_err() {
                            return; // receiver dropped
                        }
                    }
                    // Destructive read: delete the delivered message regardless of
                    // whether it decoded, so the relay does not redeliver it.
                    let _ = client.messages.acknowledge(&msg.id, &agent_id).await;
                }
            }
            tokio::time::sleep(interval).await;
        }
    });
    MailboxPoll { handle, receiver }
}

// ─── contact auto-accepter ──────────────────────────────────────────────────

/// Spawn a loop that polls incoming contact requests every `interval` and
/// accepts each one the fail-closed `allow` predicate approves (by cryptoId).
/// Requests `allow` rejects are left pending. Errors are ignored and retried.
///
/// A typical interval is ~1500ms.
pub fn spawn_contact_auto_accepter<F>(
    client: TinyPlaceClient,
    interval: Duration,
    allow: F,
) -> JoinHandle<()>
where
    F: Fn(&str) -> bool + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            if let Ok(resp) = client.contacts.requests(None).await {
                for view in resp.incoming {
                    if view.agent_id.is_empty() {
                        continue;
                    }
                    if allow(&view.agent_id) {
                        let _ = client.contacts.accept(&view.agent_id).await;
                    }
                }
            }
            tokio::time::sleep(interval).await;
        }
    })
}

// ─── presence heartbeat ─────────────────────────────────────────────────────

/// Spawn a loop that marks the acting agent online via `client.presence` every
/// `interval`. Errors are ignored and retried next tick.
pub fn spawn_presence_heartbeat(client: TinyPlaceClient, interval: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let _ = client.presence.heartbeat().await;
            tokio::time::sleep(interval).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::tinyplace_support::runtime::{load_or_create_identity, FileSessionStore};
    use tinyplace::signal::crypto::generate_x25519_keypair;
    use tinyplace::signal::keys::PreKeyPair;
    use tinyplace::signal::store::{SessionState, SessionStore};
    use tinyplace::Signer;

    /// A process/time-unique temp directory, created on demand.
    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "tinyplace-proto-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn file_session_store_round_trips_prekeys_and_sessions() {
        let dir = unique_temp_dir("store");
        let path = FileSessionStore::default_path(&dir, "@alice:agent");
        assert!(path.ends_with("signal/_alice_agent.json"));

        let identity = generate_x25519_keypair();
        let store = FileSessionStore::new(&path, identity.clone());

        // Identity key pair comes straight back.
        let got_identity = store.identity_x25519_key_pair().await.unwrap();
        assert_eq!(got_identity.public_key, identity.public_key);
        assert_eq!(got_identity.private_key, identity.private_key);

        // Store a signed pre-key (also becomes the active one) and a one-time pre-key.
        let signed = PreKeyPair {
            key_id: "spk_1".to_string(),
            key_pair: generate_x25519_keypair(),
            signature: vec![1, 2, 3, 4, 5],
        };
        store.store_signed_pre_key(signed.clone()).await.unwrap();

        let one_time = PreKeyPair {
            key_id: "pk_7".to_string(),
            key_pair: generate_x25519_keypair(),
            signature: vec![9, 8, 7],
        };
        store.store_pre_key(one_time.clone()).await.unwrap();

        // A ratchet session with skipped keys.
        let mut skipped = HashMap::new();
        skipped.insert("abc:3".to_string(), [7u8; 32]);
        let session = SessionState {
            dh_send_key_pair: generate_x25519_keypair(),
            dh_recv_public_key: Some([2u8; 32]),
            root_key: [3u8; 32],
            send_chain_key: Some([4u8; 32]),
            recv_chain_key: None,
            send_message_number: 5,
            recv_message_number: 6,
            previous_chain_length: 7,
            skipped_keys: skipped,
        };
        store.store_session("@peer", session.clone()).await.unwrap();

        // A fresh store instance reads the same file from disk (no shared cache).
        let reopened = FileSessionStore::new(&path, identity.clone());

        let active = reopened.active_signed_pre_key().await.unwrap();
        assert_eq!(active.key_id, "spk_1");
        assert_eq!(active.key_pair.public_key, signed.key_pair.public_key);
        assert_eq!(active.signature, signed.signature);

        let got_signed = reopened.signed_pre_key("spk_1").await.unwrap().unwrap();
        assert_eq!(got_signed.key_pair.private_key, signed.key_pair.private_key);

        let got_pre = reopened.pre_key("pk_7").await.unwrap().unwrap();
        assert_eq!(got_pre.key_pair.public_key, one_time.key_pair.public_key);
        assert_eq!(got_pre.signature, one_time.signature);
        assert_eq!(reopened.all_pre_keys().await.unwrap().len(), 1);

        let got_session = reopened.session("@peer").await.unwrap().unwrap();
        assert_eq!(
            got_session.dh_send_key_pair.public_key,
            session.dh_send_key_pair.public_key
        );
        assert_eq!(got_session.dh_recv_public_key, Some([2u8; 32]));
        assert_eq!(got_session.root_key, [3u8; 32]);
        assert_eq!(got_session.send_chain_key, Some([4u8; 32]));
        assert_eq!(got_session.recv_chain_key, None);
        assert_eq!(got_session.send_message_number, 5);
        assert_eq!(got_session.recv_message_number, 6);
        assert_eq!(got_session.previous_chain_length, 7);
        assert_eq!(got_session.skipped_keys.get("abc:3"), Some(&[7u8; 32]));

        // Removal persists.
        reopened.remove_pre_key("pk_7").await.unwrap();
        reopened.remove_session("@peer").await.unwrap();
        assert!(reopened.pre_key("pk_7").await.unwrap().is_none());
        assert!(reopened.session("@peer").await.unwrap().is_none());

        // The on-disk file uses the TS-compatible camelCase layout.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"signedPreKeys\""));
        assert!(raw.contains("\"activeSignedPreKeyId\""));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_or_create_identity_is_deterministic_across_calls() {
        let dir = unique_temp_dir("identity");
        let config_path = dir.join("config.json");
        let env: HashMap<String, String> = HashMap::new();

        // First call: no key anywhere, mint and persist.
        let (signer1, config1) = load_or_create_identity(&config_path, &env).unwrap();
        let agent1 = signer1.agent_id();
        let key = config1.secret_key.clone().expect("secret persisted");
        assert_eq!(key.len(), 64, "seed is 64 hex chars");
        assert!(config_path.exists(), "config written");

        // Second call: reads the persisted seed and yields the same identity.
        let (signer2, config2) = load_or_create_identity(&config_path, &env).unwrap();
        assert_eq!(signer2.agent_id(), agent1);
        assert_eq!(config2.secret_key.as_deref(), Some(key.as_str()));

        // An env-provided key overrides the file.
        let other = tinyplace::LocalSigner::generate();
        let other_hex: String = other.seed().iter().map(|b| format!("{b:02x}")).collect();
        let mut env2 = HashMap::new();
        env2.insert("TINYPLACE_SECRET_KEY".to_string(), other_hex.clone());
        let (signer3, config3) = load_or_create_identity(&config_path, &env2).unwrap();
        assert_eq!(signer3.agent_id(), other.agent_id());
        assert_eq!(config3.secret_key.as_deref(), Some(other_hex.as_str()));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
