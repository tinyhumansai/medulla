//! The one type both endpoints use: a UDP socket, a session per peer, and a
//! driver task that turns [`crate::transport::Session`]'s send policy into
//! actual datagrams.
//!
//! Roaming needs no code here. An endpoint keeps sending to the same forwarder
//! address; the *forwarder* relearns where each node is from the datagrams it
//! receives (§5 rule 5). A laptop that wakes on a different network therefore
//! resumes by sending, with no reconnect, no handshake and no re-enrollment.

mod types;

use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch, Mutex};

use crate::header::MAX_DATAGRAM;
use crate::keys::{self, AcquiredNode, NodeId};
use crate::state::QueueLimits;
use crate::transport::{Session, SessionConfig};

use types::{Command, SCREEN_FRAME_HEADER, SCREEN_FRAME_MAGIC};
pub use types::{LinkConfig, LinkError, LinkHandle, LinkStatus, PeerConfig};

/// How many messages may sit in the inbound queue before the driver blocks.
///
/// Backpressure rather than unbounded buffering: a consumer that has stopped
/// draining should slow the link down, not exhaust memory.
const INBOUND_CAPACITY: usize = 1024;

/// How many commands may be in flight from handles to the driver.
const COMMAND_CAPACITY: usize = 256;

/// Brings a link up.
pub struct Link;

impl Link {
    /// Open the identity, bind the socket, and start the driver task.
    ///
    /// The returned handle is the only way to reach the link; dropping it stops
    /// the driver, which releases the identity lock.
    ///
    /// # Errors
    ///
    /// [`LinkError::Key`] when the identity is missing, malformed or held by
    /// another process; [`LinkError::Endpoint`] when the forwarder address does
    /// not resolve; [`LinkError::Io`] when the socket cannot be bound.
    pub async fn connect(config: LinkConfig) -> Result<LinkHandle, LinkError> {
        let node = keys::acquire(&config.state_dir)?;
        let endpoint = config
            .forwarder_endpoint
            .clone()
            .unwrap_or_else(|| node.state.forwarder_endpoint.clone());
        let socket = UdpSocket::bind(config.bind).await?;
        let forwarder = resolve(&endpoint, config.bind.is_ipv4())?;

        let (command_tx, command_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (inbound_tx, inbound_rx) = mpsc::channel(INBOUND_CAPACITY);
        let (status_tx, status_rx) = watch::channel(LinkStatus::default());

        let driver = Driver::new(&config, node, socket, forwarder, inbound_tx, status_tx);
        // Captured before the driver moves into its task: the peer table is
        // fixed at connect, and a caller that needs it must not have to wait for
        // the first status publication to see it.
        let peers = driver.sessions.keys().copied().collect();
        let screen_updates = driver
            .sessions
            .keys()
            .copied()
            .map(|peer| (peer, Mutex::new(())))
            .collect();
        tokio::spawn(driver.run(command_rx));

        Ok(LinkHandle {
            commands: command_tx,
            inbound: Mutex::new(inbound_rx),
            status: status_rx,
            peers,
            screen_updates,
        })
    }
}

/// Resolve a `host:port` forwarder endpoint to one address.
fn resolve(endpoint: &str, ipv4: bool) -> Result<SocketAddr, LinkError> {
    endpoint
        .to_socket_addrs()
        .map_err(|_| LinkError::Endpoint(endpoint.to_string()))?
        .find(|address| address.is_ipv4() == ipv4)
        .ok_or_else(|| LinkError::Endpoint(endpoint.to_string()))
}

/// The task that owns the socket, the sessions and the sequence source.
struct Driver {
    socket: UdpSocket,
    forwarder: SocketAddr,
    sessions: HashMap<NodeId, Session>,
    node: AcquiredNode,
    inbound: mpsc::Sender<(NodeId, u64, Vec<u8>)>,
    pending_inbound: HashMap<NodeId, VecDeque<(u64, Vec<u8>)>>,
    inbound_cursor: usize,
    last_screens: HashMap<NodeId, Vec<Vec<u8>>>,
    status: watch::Sender<LinkStatus>,
    /// Monotonic origin: every `now_ms` in the transport is measured from here,
    /// so a wall-clock jump cannot make a retransmission timer fire late (or
    /// never).
    origin: Instant,
}

impl Driver {
    /// Build the driver and its one session per configured peer.
    fn new(
        config: &LinkConfig,
        node: AcquiredNode,
        socket: UdpSocket,
        forwarder: SocketAddr,
        inbound: mpsc::Sender<(NodeId, u64, Vec<u8>)>,
        status: watch::Sender<LinkStatus>,
    ) -> Self {
        let origin = Instant::now();
        let peers = if config.peers.is_empty() {
            node.state
                .enrolled_peers()
                .into_iter()
                .map(|peer| PeerConfig {
                    node_id: peer.node_id,
                    pair_key: peer.pair_key,
                })
                .collect()
        } else {
            config.peers.clone()
        };
        let sessions = peers
            .into_iter()
            .map(|peer| {
                let session = Session::new(
                    session_config(&node, &peer, config.queue_limits, config.max_sent_states),
                    0,
                );
                (peer.node_id, session)
            })
            .collect();
        Driver {
            socket,
            forwarder,
            sessions,
            node,
            inbound,
            pending_inbound: HashMap::new(),
            inbound_cursor: 0,
            last_screens: HashMap::new(),
            status,
            origin,
        }
    }

    /// Milliseconds since the driver started, the transport's whole notion of time.
    fn now_ms(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }

    /// Drive the link until every handle is dropped.
    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        let mut buffer = vec![0u8; MAX_DATAGRAM];
        loop {
            self.flush_inbound();
            self.flush().await;
            self.publish_status();

            let now = self.now_ms();
            let wake = self
                .sessions
                .values()
                .map(|session| session.next_send_ms(now))
                .min()
                .unwrap_or(now + 1_000);
            let delay = Duration::from_millis(wake.saturating_sub(now).max(1));

            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.handle_command(command),
                    // Every handle is gone: stop, releasing the identity lock.
                    None => return,
                },
                received = self.socket.recv_from(&mut buffer) => {
                    if let Ok((len, from)) = received {
                        if from != self.forwarder {
                            continue;
                        }
                        self.handle_datagram(&buffer[..len]);
                    }
                }
                _ = tokio::time::sleep(delay) => {}
            }
        }
    }

    /// Apply a handle's request to the right session.
    fn handle_command(&mut self, command: Command) {
        match command {
            Command::Send { peer, body, reply } => {
                let result = match self.sessions.get_mut(&peer) {
                    Some(session) => session.queue_message(body).map_err(LinkError::Transport),
                    None => Err(LinkError::UnknownPeer(peer)),
                };
                let _ = reply.send(result);
            }
            Command::Screen { peer, rows, reply } => {
                let result = match self.sessions.get_mut(&peer) {
                    Some(session) => session.set_screen(rows).map_err(LinkError::Transport),
                    None => Err(LinkError::UnknownPeer(peer)),
                };
                let _ = reply.send(result);
            }
        }
    }

    /// Feed one received datagram to the session it belongs to.
    ///
    /// A datagram that fails to decode, authenticate or address correctly is
    /// dropped. That is not an error condition: with a blind forwarder in the
    /// middle, junk arriving is expected, and SSP recovers from a dropped
    /// datagram by construction.
    fn handle_datagram(&mut self, datagram: &[u8]) {
        let now = self.now_ms();
        let Ok(header) = crate::header::OuterHeader::decode(datagram) else {
            return;
        };
        let Some(session) = self.sessions.get_mut(&header.src) else {
            return;
        };
        let pending = self.pending_inbound.entry(header.src).or_default();
        if !pending.is_empty() {
            return;
        }
        if session.handle_datagram(datagram, now).is_err() {
            return;
        }
        let messages = session.take_messages();
        let epoch = session
            .peer_epoch()
            .expect("an accepted datagram has an epoch");
        for message in messages {
            pending.push_back((epoch, message));
        }
        let screen = session.screen().rows().to_vec();
        if self.last_screens.get(&header.src) != Some(&screen) {
            self.last_screens.insert(header.src, screen.clone());
            if let Some(message) = complete_screen_frame(&screen) {
                pending.push_back((epoch, message));
            }
        }
    }

    /// Move pending deliveries into the shared handle queue without ever
    /// suspending the socket/session driver on a slow consumer.
    fn flush_inbound(&mut self) {
        let peers: Vec<NodeId> = self.pending_inbound.keys().copied().collect();
        if peers.is_empty() {
            return;
        }
        let start = self.inbound_cursor % peers.len();
        for offset in 0..peers.len() {
            let index = (start + offset) % peers.len();
            let peer = peers[index];
            let Some(pending) = self.pending_inbound.get_mut(&peer) else {
                continue;
            };
            let Some((epoch, message)) = pending.pop_front() else {
                continue;
            };
            match self.inbound.try_send((peer, epoch, message)) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full((_, epoch, message))) => {
                    pending.push_front((epoch, message));
                    self.inbound_cursor = (index + 1) % peers.len();
                    return;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return,
            }
        }
        self.inbound_cursor = (start + 1) % peers.len();
    }

    /// Send whatever every session says is due now.
    async fn flush(&mut self) {
        let now = self.now_ms();
        let forwarder = self.forwarder;
        // Destructured because the sessions and the sequence source are separate
        // fields: the borrow checker cannot see that through `self`.
        let Driver {
            sessions,
            node,
            socket,
            ..
        } = self;
        for session in sessions.values_mut() {
            let datagrams = match session.outgoing(now, &mut node.seq) {
                Ok(datagrams) => datagrams,
                // A state change too large to send, or a sequence that could not
                // be reserved. Neither is fixed by trying again this millisecond;
                // the next state change re-attempts.
                Err(_) => continue,
            };
            for datagram in datagrams {
                let _ = socket.send_to(&datagram, forwarder).await;
            }
        }
    }

    /// Publish a fresh status snapshot for `LinkHandle::status`.
    fn publish_status(&self) {
        let now = self.now_ms();
        let peers = self
            .sessions
            .iter()
            .map(|(id, session)| (*id, session.status(now)))
            .collect();
        let _ = self.status.send(LinkStatus { peers });
    }
}

/// Reassemble the application frame once every channel-1 row has arrived.
fn complete_screen_frame(rows: &[Vec<u8>]) -> Option<Vec<u8>> {
    if rows.is_empty() {
        return None;
    }
    let count = rows.len();
    let mut body = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if row.len() < SCREEN_FRAME_HEADER || &row[..4] != SCREEN_FRAME_MAGIC {
            return None;
        }
        let declared = u32::from_be_bytes(row[4..8].try_into().ok()?) as usize;
        let declared_index = u32::from_be_bytes(row[8..12].try_into().ok()?) as usize;
        if declared != count || declared_index != index {
            return None;
        }
        body.extend_from_slice(&row[SCREEN_FRAME_HEADER..]);
    }
    Some(body)
}

/// Build a session configuration from the node identity and one peer.
fn session_config(
    node: &AcquiredNode,
    peer: &PeerConfig,
    queue_limits: QueueLimits,
    max_sent_states: usize,
) -> SessionConfig {
    SessionConfig {
        node_id: node.state.node_id,
        peer_node_id: peer.node_id,
        role: node.state.role,
        pair_key: peer.pair_key.clone(),
        forwarder_key: node.state.forwarder_key.clone(),
        queue_limits,
        max_sent_states,
    }
}
