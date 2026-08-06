//! Data model for a node's persisted link identity.
//!
//! Everything here is what `<home>/link/node.json` holds (protocol §7.3): the
//! node id that travels on the wire, the role that fixes the direction bit, the
//! two secrets, the forwarder endpoint, and the sequence reservation of §3.1.

use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::pair_key::PairKey;

/// Errors from loading, minting, or persisting link key material.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// The state file, its directory, or its lock could not be read or written.
    #[error("link identity i/o: {0}")]
    Io(#[from] std::io::Error),
    /// The state file exists but does not parse as a `node.json`.
    #[error("link identity at {path} is malformed: {reason}")]
    Malformed {
        /// The file that failed to parse.
        path: PathBuf,
        /// The underlying parse complaint.
        reason: String,
    },
    /// No state file: this endpoint has not enrolled yet.
    #[error("no link identity at {0} — this endpoint has not enrolled")]
    NotEnrolled(PathBuf),
    /// Another process holds the identity lock.
    #[error("link identity {0} is already in use by another process")]
    Busy(PathBuf),
}

/// Result alias for key operations.
pub type KeyResult<T> = Result<T, KeyError>;

/// Which end of the link this node is. Fixed at enrollment (protocol §2).
///
/// The role is what selects the direction bit of §4.2, so the two endpoints draw
/// their AEAD nonces from disjoint halves of the counter space and a single pair
/// key is safe in both directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The medulla orchestrator. Sets the direction bit.
    Orchestrator,
    /// A remote host. Clears the direction bit.
    Host,
}

impl Role {
    /// The §4.2 direction bit for datagrams **sent** by a node in this role.
    pub fn direction_bit(self) -> u64 {
        match self {
            Role::Orchestrator => crate::crypto::DIRECTION_MASK,
            Role::Host => 0,
        }
    }

    /// The role at the other end of the link.
    pub fn peer(self) -> Role {
        match self {
            Role::Orchestrator => Role::Host,
            Role::Host => Role::Orchestrator,
        }
    }
}

/// The 16 random bytes the backend issues at enrollment (protocol §2).
///
/// This is the only identifier that travels on the wire; `node_name` stays in
/// the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub [u8; 16]);

impl NodeId {
    /// Mint a random node id. Endpoints do not normally do this — the backend
    /// issues ids — but tests and local loopback links need one.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
        NodeId(bytes)
    }

    /// The raw bytes, as they appear in the outer header.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Decode the 32-character hexadecimal form issued by enrollment.
    pub fn from_hex(text: &str) -> Option<Self> {
        let bytes: [u8; 16] = hex_decode(text)?.try_into().ok()?;
        Some(NodeId(bytes))
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex_encode(&self.0))
    }
}

impl Serialize for NodeId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex_encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for NodeId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes =
            hex_decode(&text).ok_or_else(|| D::Error::custom("node id is not 16 hex bytes"))?;
        let bytes: [u8; 16] = bytes
            .try_into()
            .map_err(|_| D::Error::custom("node id is not 16 bytes"))?;
        Ok(NodeId(bytes))
    }
}

/// The 32-byte HMAC key the backend issues at enrollment.
///
/// It authenticates the cleartext outer header and nothing else — it cannot
/// decrypt a payload, which is the property the blind-forwarder model rests on.
#[derive(Clone, PartialEq, Eq)]
pub struct ForwarderKey(pub [u8; 32]);

impl ForwarderKey {
    /// Mint a random forwarder key (tests and loopback links).
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
        ForwarderKey(bytes)
    }

    /// The raw key bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for ForwarderKey {
    /// Never prints the key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ForwarderKey(<redacted>)")
    }
}

impl Serialize for ForwarderKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex_encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for ForwarderKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes =
            hex_decode(&text).ok_or_else(|| D::Error::custom("forwarder key is not valid hex"))?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| D::Error::custom("forwarder key is not 32 bytes"))?;
        Ok(ForwarderKey(bytes))
    }
}

/// The persisted contents of `<home>/link/node.json` (protocol §7.3).
///
/// The file is written `0600` and holds a pair key, so it is the one file in a
/// medulla home whose disclosure breaks confidentiality. It is never logged and
/// its `Debug` is redacted.
#[derive(Clone, Serialize, Deserialize)]
pub struct NodeState {
    /// Schema version of this file. `1`.
    pub version: u32,
    /// This node's wire identifier.
    pub node_id: NodeId,
    /// Orchestrator or host — fixes the direction bit.
    pub role: Role,
    /// The end-to-end key. Never leaves this file and the user's hands.
    pub pair_key: PairKey,
    /// The outer-header HMAC key, shared with the backend.
    pub forwarder_key: ForwarderKey,
    /// `host:port` of the forwarder to send datagrams to.
    pub forwarder_endpoint: String,
    /// The node id of the peer this link talks to (the orchestrator, for a host).
    pub peer_node_id: NodeId,
    /// Every enrolled peer and its distinct pair key. Empty in version-1 files,
    /// which fall back to `peer_node_id` and `pair_key` above.
    #[serde(default)]
    pub peers: Vec<EnrolledPeer>,
    /// The §3.1 reservation: every sequence **below** this value must be
    /// considered spent, whatever this process last used.
    pub seq_reservation: u64,
}

/// Secret session material for one enrolled peer.
#[derive(Clone, Serialize, Deserialize)]
pub struct EnrolledPeer {
    /// The peer's wire identifier.
    pub node_id: NodeId,
    /// The pair-specific end-to-end key.
    pub pair_key: PairKey,
}

impl NodeState {
    /// Return every enrolled peer, including the legacy single-peer fields.
    pub fn enrolled_peers(&self) -> Vec<EnrolledPeer> {
        if self.peers.is_empty() {
            vec![EnrolledPeer {
                node_id: self.peer_node_id,
                pair_key: self.pair_key.clone(),
            }]
        } else {
            self.peers.clone()
        }
    }
}

impl fmt::Debug for NodeState {
    /// Names the secrets without printing them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeState")
            .field("version", &self.version)
            .field("node_id", &self.node_id)
            .field("role", &self.role)
            .field("forwarder_endpoint", &self.forwarder_endpoint)
            .field("peer_node_id", &self.peer_node_id)
            .field("seq_reservation", &self.seq_reservation)
            .finish_non_exhaustive()
    }
}

/// An exclusive hold on a node's identity directory.
///
/// The lock is an advisory whole-file lock on `<home>/link/node.lock`, the same
/// construction the host-link identity bootstrap uses: it lives as long as this
/// guard, and the OS releases it when the holding process dies, so a crashed
/// endpoint leaves its identity immediately reclaimable without a pidfile dance.
///
/// **Keep it alive for the process lifetime.** Dropping it lets a second process
/// open the same node and hand out sequences from its own reservation.
pub struct NodeLock {
    file: File,
    path: PathBuf,
}

impl NodeLock {
    /// Wrap an already-locked file. Constructed only by the acquisition path.
    pub(super) fn new(file: File, path: PathBuf) -> Self {
        NodeLock { file, path }
    }

    /// The lock file this guard holds.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for NodeLock {
    fn drop(&mut self) {
        // Closing would release it anyway; unlocking explicitly makes a released
        // identity observable in tests without relying on close ordering.
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

impl fmt::Debug for NodeLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeLock")
            .field("path", &self.path)
            .finish()
    }
}

/// Lowercase hex, the encoding every byte string in `node.json` uses.
pub(super) fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Decode lowercase or uppercase hex; `None` on odd length or a non-hex digit.
pub(super) fn hex_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok())
        .collect()
}
