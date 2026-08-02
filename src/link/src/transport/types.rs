//! Transport errors and session configuration.

use crate::crypto::CryptoError;
use crate::header::HeaderError;
use crate::keys::{ForwarderKey, KeyError, NodeId, PairKey, Role};
use crate::packet::PacketError;
use crate::state::{QueueLimits, StateError};

use super::channel::MAX_SENT_STATES;

/// Why a datagram could not be built, sent, or accepted.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The outbound queue or the sent-state history has reached its bound.
    ///
    /// **Retryable.** The usual cause is a peer that has been unreachable for a
    /// long time; the link is still healthy and the same message will go through
    /// once the peer catches up.
    #[error("link queue overflow: {0}")]
    QueueOverflow(String),
    /// A single state change is larger than one datagram can carry (§3.2).
    ///
    /// Not retryable: the application must produce a smaller change.
    #[error("state diff of {size} bytes exceeds the {limit}-byte datagram budget")]
    DiffTooLarge {
        /// The diff that did not fit.
        size: usize,
        /// The per-datagram budget.
        limit: usize,
    },
    /// A datagram whose source or destination is not this link's pair.
    ///
    /// Dropped rather than acted on: with a blind forwarder, a misaddressed
    /// datagram is either a forwarder bug or an attempt to confuse us.
    #[error("datagram is not for this link: {0}")]
    Misaddressed(String),
    /// The state object refused the diff.
    #[error(transparent)]
    State(#[from] StateError),
    /// The plaintext framing did not decode.
    #[error(transparent)]
    Packet(#[from] PacketError),
    /// The outer header did not decode.
    #[error(transparent)]
    Header(#[from] HeaderError),
    /// The payload did not authenticate.
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    /// A sequence could not be allocated or persisted.
    #[error(transparent)]
    Key(#[from] KeyError),
}

impl TransportError {
    /// Whether the caller should retry rather than fail the work.
    ///
    /// Only queue pressure is retryable: it means "the peer is behind", which
    /// resolves itself. Everything else is a malformed or hostile datagram, or a
    /// change the application must make smaller.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            TransportError::QueueOverflow(_) | TransportError::State(StateError::Overflow(_))
        )
    }
}

/// Everything a [`super::Session`] needs to talk to exactly one peer.
pub struct SessionConfig {
    /// This endpoint's node id.
    pub node_id: NodeId,
    /// The peer's node id.
    pub peer_node_id: NodeId,
    /// This endpoint's role, which fixes the direction bit of §4.2.
    pub role: Role,
    /// The end-to-end key. Both endpoints hold the same one.
    pub pair_key: PairKey,
    /// This endpoint's outer-header key, shared with the forwarder.
    pub forwarder_key: ForwarderKey,
    /// Bound on the outbound message queue (§4.5).
    pub queue_limits: QueueLimits,
    /// Bound on retained sent states per channel.
    pub max_sent_states: usize,
}

impl SessionConfig {
    /// A configuration with the default bounds.
    pub fn new(
        node_id: NodeId,
        peer_node_id: NodeId,
        role: Role,
        pair_key: PairKey,
        forwarder_key: ForwarderKey,
    ) -> Self {
        SessionConfig {
            node_id,
            peer_node_id,
            role,
            pair_key,
            forwarder_key,
            queue_limits: QueueLimits::default(),
            max_sent_states: MAX_SENT_STATES,
        }
    }
}
