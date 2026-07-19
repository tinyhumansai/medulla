//! Data model for the tiny.place agent-runtime helpers.
//!
//! Two kinds of types live here: the public error/result and mailbox surface the
//! helpers hand back to callers, and the private on-disk JSON shapes the
//! [`FileSessionStore`](super::session_store::FileSessionStore) reads and writes.
//! The persistence shapes and their fields are `pub(super)` so the store logic in
//! [`session_store`](super::session_store) can convert them to and from
//! signal-crate types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use tinyplace::error::Error as SdkError;
use tinyplace::types::MessageEnvelope;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::super::frames::TaskFrame;

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
    /// Handle to the background poll task.
    pub handle: JoinHandle<()>,
    /// Receiving end of the decoded-message channel.
    pub receiver: mpsc::Receiver<MailboxItem>,
}

// On-disk JSON shapes. Field names are camelCase and the layout mirrors the TS
// SDK's `FileSessionStore` (`version`, `signedPreKeys`, `activeSignedPreKeyId`,
// `preKeys`, `sessions`), so a file this store writes is readable by the TS CLI
// and vice-versa. Every byte array is base64; the skipped-key map is an array of
// `[id, base64]` pairs. NOTE: the Rust SDK's `SessionStore` trait has no group
// sender-key methods, so this store neither reads nor writes `senderKeysOwn` /
// `senderKeyReceivers`; rewriting a file that the TS group layer populated would
// drop those blocks. Group messaging is out of scope for the medulla runtime.

/// A base64-encoded X25519 key pair, as stored on disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SerializedKeyPair {
    pub(super) public_key: String,
    pub(super) private_key: String,
}

/// A stored pre-key: its id, key pair, and base64 signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SerializedPreKey {
    pub(super) key_id: String,
    pub(super) key_pair: SerializedKeyPair,
    pub(super) signature: String,
}

/// A stored ratchet session, mirroring the signal crate's `SessionState`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SerializedSession {
    pub(super) dh_send_key_pair: SerializedKeyPair,
    pub(super) dh_recv_public_key: Option<String>,
    pub(super) root_key: String,
    pub(super) send_chain_key: Option<String>,
    pub(super) recv_chain_key: Option<String>,
    pub(super) send_message_number: u32,
    pub(super) recv_message_number: u32,
    pub(super) previous_chain_length: u32,
    pub(super) skipped_keys: Vec<(String, String)>,
}

/// The whole on-disk document for one identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PersistShape {
    #[serde(default = "one")]
    pub(super) version: u8,
    #[serde(default)]
    pub(super) signed_pre_keys: BTreeMap<String, SerializedPreKey>,
    #[serde(default)]
    pub(super) active_signed_pre_key_id: Option<String>,
    #[serde(default)]
    pub(super) pre_keys: BTreeMap<String, SerializedPreKey>,
    #[serde(default)]
    pub(super) sessions: BTreeMap<String, SerializedSession>,
}

/// serde default for [`PersistShape::version`].
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
