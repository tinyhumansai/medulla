//! Configuration, handle and error types for a running link.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use tokio::sync::{mpsc, oneshot, watch, Mutex};

use crate::keys::{KeyError, NodeId, PairKey};
use crate::state::QueueLimits;
use crate::transport::{SessionStatus, TransportError, MAX_SENT_STATES};

pub(super) const SCREEN_FRAME_MAGIC: &[u8; 4] = b"MSF1";
pub(super) const SCREEN_FRAME_HEADER: usize = 12;

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

/// Where a link's datagrams go, and what authenticates the outer header.
///
/// The two variants are the two ways a peer is reached, and they differ in more
/// than an address: see [`crate::link::direct`] for what a forwarder was doing
/// that a direct link has to do for itself.
#[derive(Debug, Clone)]
pub enum LinkPath {
    /// Through the backend forwarder — the enrolled default. Outer headers are
    /// tagged with this node's forwarder key and never verified locally, because
    /// the receiver has never held the sender's key.
    Forwarder {
        /// Overrides the forwarder endpoint recorded in `node.json`.
        endpoint: Option<String>,
    },
    /// Straight to one peer, at an address an SSH bootstrap supplied. There is
    /// no third party, so the outer tag is derived from the pair key and *is*
    /// verified, and the peer's address is learned rather than relayed.
    Direct {
        /// `host:port` to start sending to. Roaming may move it from here.
        endpoint: String,
    },
}

/// How to bring a link up.
#[derive(Debug, Clone)]
pub struct LinkConfig {
    /// The identity directory, normally `<medulla_home>/link`.
    ///
    /// One process may hold only one link per directory: `keys::acquire` takes an
    /// advisory lock, so a second [`Link`](crate::Link) on the same directory
    /// fails with [`KeyError`]. A client that talks to several remote hosts
    /// therefore needs a directory per host.
    pub state_dir: PathBuf,
    /// Local UDP address to bind. `0.0.0.0:0` is the usual choice: the forwarder
    /// learns the endpoint's address from the datagrams it receives (§5), so
    /// nothing depends on the local port staying the same — that is what makes
    /// roaming free.
    pub bind: SocketAddr,
    /// How the peer is reached.
    pub path: LinkPath,
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
            path: LinkPath::Forwarder { endpoint: None },
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
    /// A direct link was configured with anything other than exactly one peer.
    ///
    /// Refused rather than approximated: a direct link has one socket pointed at
    /// one address, so a second peer would silently receive the first one's
    /// datagrams. The forwarded path is the one that fans out.
    #[error("a direct link needs exactly one peer, got {0}")]
    DirectPeerCount(usize),
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
    /// The address the driver's socket is bound to.
    ///
    /// Captured at connect rather than asked of the socket later, because the
    /// socket moves into the driver task. A direct-mode host needs it: it binds
    /// an ephemeral port and has to tell the client which one it got.
    pub(super) local_addr: SocketAddr,
    pub(super) commands: mpsc::Sender<Command>,
    pub(super) inbound: Mutex<mpsc::Receiver<(NodeId, u64, Vec<u8>)>>,
    pub(super) status: watch::Receiver<LinkStatus>,
    pub(super) peers: Vec<NodeId>,
    pub(super) screen_updates: HashMap<NodeId, Mutex<()>>,
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

    /// Replace `peer`'s screen frame, splitting large frames into state steps.
    pub async fn send_screen_frame(&self, peer: NodeId, body: &[u8]) -> Result<(), LinkError> {
        let screen_update = self
            .screen_updates
            .get(&peer)
            .ok_or(LinkError::UnknownPeer(peer))?;
        // A frame is represented by several cumulative grid states. Keep those
        // states contiguous: interleaving another producer's prefixes could
        // create a transition whose changed rows no longer fit one datagram.
        let _guard = screen_update.lock().await;
        let capacity = crate::transport::MAX_MESSAGE_BYTES - 20;
        let count = body.len().div_ceil(capacity).max(1);
        let mut rows = Vec::with_capacity(count);
        for (index, chunk) in body.chunks(capacity).enumerate() {
            let mut row = Vec::with_capacity(SCREEN_FRAME_HEADER + chunk.len());
            row.extend_from_slice(SCREEN_FRAME_MAGIC);
            row.extend_from_slice(&(count as u32).to_be_bytes());
            row.extend_from_slice(&(index as u32).to_be_bytes());
            row.extend_from_slice(chunk);
            rows.push(row);
            self.send_screen(peer, rows.clone()).await?;
        }
        if body.is_empty() {
            let mut row = Vec::with_capacity(SCREEN_FRAME_HEADER);
            row.extend_from_slice(SCREEN_FRAME_MAGIC);
            row.extend_from_slice(&1u32.to_be_bytes());
            row.extend_from_slice(&0u32.to_be_bytes());
            self.send_screen(peer, vec![row]).await?;
        }
        Ok(())
    }

    /// The next message received from any peer.
    ///
    /// `None` means the driver has stopped and no further message will arrive.
    pub async fn recv(&self) -> Option<(NodeId, u64, Vec<u8>)> {
        self.inbound.lock().await.recv().await
    }

    /// The local address this link's socket is bound to.
    ///
    /// With an ephemeral `bind` (the usual choice) this is the only way to learn
    /// the port that was actually assigned.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Every peer this link opened a session with.
    ///
    /// The host case has exactly one — the orchestrator recorded in `node.json`
    /// — and a caller that must map a name onto a node id has no other source
    /// for it, since the name lives only in the backend registry (§2).
    pub fn peers(&self) -> &[NodeId] {
        &self.peers
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
