//! Configuration, handle and error types for a running link.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use tokio::sync::{mpsc, oneshot, watch, Mutex};

use crate::keys::{KeyError, NodeId, PairKey};
use crate::state::QueueLimits;
use crate::transport::{SessionStatus, TransportError, MAX_SENT_STATES};

/// A peer this endpoint can talk to, and the key it is talked to with.
///
/// The pair key is per peer by construction (§7.1): an orchestrator generates
/// one key per host, so compromising one host's key tells an attacker nothing
/// about any other host.
#[derive(Debug, Clone)]
pub struct PeerConfig {
    /// The peer's node id, as issued by the backend at enrollment.
    pub node_id: NodeId,
    /// The end-to-end key shared with that peer.
    pub pair_key: PairKey,
}

/// How to bring a link up.
#[derive(Debug, Clone)]
pub struct LinkConfig {
    /// The identity directory, normally `<medulla_home>/link`.
    pub state_dir: PathBuf,
    /// Local UDP address to bind. `0.0.0.0:0` is the usual choice: the forwarder
    /// learns the endpoint's address from the datagrams it receives (§5), so
    /// nothing depends on the local port staying the same — that is what makes
    /// roaming free.
    pub bind: SocketAddr,
    /// Overrides the forwarder endpoint recorded in `node.json`.
    pub forwarder_endpoint: Option<String>,
    /// Peers to open sessions with. When empty, the single peer recorded in
    /// `node.json` is used — the host case, which has exactly one.
    pub peers: Vec<PeerConfig>,
    /// Bound on each peer's outbound queue (§4.5).
    pub queue_limits: QueueLimits,
    /// Bound on retained sent states per channel.
    pub max_sent_states: usize,
}

impl LinkConfig {
    /// A configuration with the usual defaults: ephemeral local port, the
    /// forwarder and peer from `node.json`, default bounds.
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        LinkConfig {
            state_dir: state_dir.into(),
            bind: "0.0.0.0:0"
                .parse()
                .expect("a literal socket address parses"),
            forwarder_endpoint: None,
            peers: Vec::new(),
            queue_limits: QueueLimits::default(),
            max_sent_states: MAX_SENT_STATES,
        }
    }
}

/// Why a link operation failed.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// Identity material could not be loaded, minted or locked.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// The socket failed, or the forwarder endpoint did not resolve.
    #[error("link i/o: {0}")]
    Io(#[from] std::io::Error),
    /// The forwarder endpoint is not a resolvable `host:port`.
    #[error("forwarder endpoint {0:?} does not resolve")]
    Endpoint(String),
    /// The state machine refused the operation.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// No session exists for that node id.
    #[error("no session for peer {0}")]
    UnknownPeer(NodeId),
    /// The driver task has stopped; the handle is dead.
    #[error("link is closed")]
    Closed,
}

impl LinkError {
    /// Whether the caller should retry rather than fail the work.
    ///
    /// Queue pressure means "the peer is behind", which resolves itself once the
    /// link recovers — the case the existing task retry path already handles.
    pub fn is_retryable(&self) -> bool {
        matches!(self, LinkError::Transport(error) if error.is_retryable())
    }
}

/// A snapshot of every peer's link state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkStatus {
    /// Per-peer liveness and timing (§6.2).
    pub peers: HashMap<NodeId, SessionStatus>,
}

impl LinkStatus {
    /// The status of one peer, if the link has a session for it.
    pub fn peer(&self, node_id: &NodeId) -> Option<&SessionStatus> {
        self.peers.get(node_id)
    }
}

/// What the handle asks the driver to do.
pub(super) enum Command {
    /// Queue a message on channel 0 for a peer.
    Send {
        /// Destination.
        peer: NodeId,
        /// Message body.
        body: Vec<u8>,
        /// Carries back the queueing result, including a retryable overflow.
        reply: oneshot::Sender<Result<(), LinkError>>,
    },
    /// Replace a peer's channel-1 grid (latest wins).
    Screen {
        /// Destination.
        peer: NodeId,
        /// The whole current grid.
        rows: Vec<Vec<u8>>,
        /// Carries back the result.
        reply: oneshot::Sender<Result<(), LinkError>>,
    },
}

/// A running link.
///
/// Cloning is deliberately not provided: the handle owns the inbound queue, and
/// two owners would race over which one receives a message. Share it behind an
/// `Arc` instead — every method takes `&self`.
#[derive(Debug)]
pub struct LinkHandle {
    pub(super) commands: mpsc::Sender<Command>,
    pub(super) inbound: Mutex<mpsc::Receiver<(NodeId, Vec<u8>)>>,
    pub(super) status: watch::Receiver<LinkStatus>,
}

impl LinkHandle {
    /// Queue `body` for `peer` on the reliable channel.
    ///
    /// Returns once the message is in the outbound state, not once it is
    /// delivered: SSP owns delivery, and there is no ack window at this layer.
    ///
    /// # Errors
    ///
    /// [`LinkError::UnknownPeer`], [`LinkError::Closed`], or a
    /// [`LinkError::Transport`] — check [`LinkError::is_retryable`] before
    /// failing the work, because a full queue is a peer that is behind, not a
    /// broken link.
    pub async fn send(&self, peer: NodeId, body: &[u8]) -> Result<(), LinkError> {
        self.request(|reply| Command::Send {
            peer,
            body: body.to_vec(),
            reply,
        })
        .await
    }

    /// Replace `peer`'s screen grid on the latest-wins channel.
    ///
    /// # Errors
    ///
    /// As [`LinkHandle::send`].
    pub async fn send_screen(&self, peer: NodeId, rows: Vec<Vec<u8>>) -> Result<(), LinkError> {
        self.request(|reply| Command::Screen { peer, rows, reply })
            .await
    }

    /// The next message received from any peer.
    ///
    /// `None` means the driver has stopped and no further message will arrive.
    pub async fn recv(&self) -> Option<(NodeId, Vec<u8>)> {
        self.inbound.lock().await.recv().await
    }

    /// The current per-peer link status (§6.2).
    ///
    /// Liveness is advisory. A peer reported `Offline` is still being
    /// retransmitted to, and needs no reconnect when it returns — callers above
    /// the link must not treat it as terminal, only as a reason to pause their
    /// own clocks (§6.3).
    pub fn status(&self) -> LinkStatus {
        self.status.borrow().clone()
    }

    /// Send a command and await its reply.
    async fn request(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<(), LinkError>>) -> Command,
    ) -> Result<(), LinkError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(make(reply))
            .await
            .map_err(|_| LinkError::Closed)?;
        answer.await.map_err(|_| LinkError::Closed)?
    }
}
