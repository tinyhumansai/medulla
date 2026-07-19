//! Filesystem-backed [`SessionStore`] persistence for Signal ratchet/pre-key
//! state, laid out to interoperate with the TS SDK's `FileSessionStore`.
//!
//! This module owns the [`FileSessionStore`] adapter, the atomic-write plumbing,
//! and the serde ↔ signal-crate conversions between the on-disk shapes in
//! [`types`](super::types) and the live key material.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;

use tinyplace::error::{Error as SdkError, Result as SdkResult};
use tinyplace::signal::crypto::X25519KeyPair;
use tinyplace::signal::keys::{PreKeyPair, SignedPreKeyPair};
use tinyplace::signal::store::{SessionState, SessionStore};

use super::types::{
    PersistShape, RuntimeError, RuntimeResult, SerializedKeyPair, SerializedPreKey,
    SerializedSession,
};

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

    /// Read the on-disk document, treating a missing file as the empty default.
    fn load(&self) -> RuntimeResult<PersistShape> {
        match std::fs::read_to_string(&self.path) {
            Ok(contents) => Ok(serde_json::from_str(&contents)?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(PersistShape::default()),
            Err(err) => Err(RuntimeError::Io(err)),
        }
    }

    /// Atomically rewrite the on-disk document (temp file + rename, `0600`).
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

/// Restrict a file to owner-only read/write (`0600`) on Unix.
#[cfg(unix)]
fn set_owner_only(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// No-op on platforms without Unix permission bits.
#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

// serde <-> signal-type conversions.

/// Encode an X25519 key pair to its base64 on-disk shape.
fn ser_key_pair(kp: &X25519KeyPair) -> SerializedKeyPair {
    SerializedKeyPair {
        public_key: BASE64.encode(kp.public_key),
        private_key: BASE64.encode(kp.private_key),
    }
}

/// Decode a base64 on-disk key pair back to an X25519 key pair.
fn de_key_pair(value: &SerializedKeyPair) -> RuntimeResult<X25519KeyPair> {
    Ok(X25519KeyPair {
        public_key: de_b32(&value.public_key)?,
        private_key: de_b32(&value.private_key)?,
    })
}

/// Encode a pre-key (signed or one-time) to its on-disk shape.
fn ser_pre_key(pre_key: &PreKeyPair) -> SerializedPreKey {
    SerializedPreKey {
        key_id: pre_key.key_id.clone(),
        key_pair: ser_key_pair(&pre_key.key_pair),
        signature: BASE64.encode(&pre_key.signature),
    }
}

/// Decode an on-disk pre-key back to a [`PreKeyPair`].
fn de_pre_key(value: &SerializedPreKey) -> RuntimeResult<PreKeyPair> {
    Ok(PreKeyPair {
        key_id: value.key_id.clone(),
        key_pair: de_key_pair(&value.key_pair)?,
        signature: BASE64
            .decode(value.signature.as_bytes())
            .map_err(|err| RuntimeError::Invalid(format!("invalid base64 signature: {err}")))?,
    })
}

/// Encode a ratchet session to its on-disk shape.
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

/// Decode an on-disk session back to a [`SessionState`].
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

/// Decode a base64 string into exactly 32 bytes, rejecting wrong lengths.
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
