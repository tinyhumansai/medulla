//! The one type both endpoints use: a UDP socket, a session per peer, and a
//! driver task that turns [`crate::transport::Session`]'s send policy into
//! actual datagrams.
//!
//! Roaming needs no code here. An endpoint keeps sending to the same forwarder
//! address; the *forwarder* relearns where each node is from the datagrams it
//! receives (§5 rule 5). A laptop that wakes on a different network therefore
//! resumes by sending, with no reconnect, no handshake and no re-enrollment.

mod types;

use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch, Mutex};

use crate::header::MAX_DATAGRAM;
use crate::keys::{self, AcquiredNode, NodeId};
use crate::state::QueueLimits;
use crate::transport::{Session, SessionConfig};

use types::Command;
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
        let forwarder = resolve(&endpoint)?;
        let socket = UdpSocket::bind(config.bind).await?;

        let (command_tx, command_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (inbound_tx, inbound_rx) = mpsc::channel(INBOUND_CAPACITY);
        let (status_tx, status_rx) = watch::channel(LinkStatus::default());

        let driver = Driver::new(&config, node, socket, forwarder, inbound_tx, status_tx);
        tokio::spawn(driver.run(command_rx));

        Ok(LinkHandle {
            commands: command_tx,
            inbound: Mutex::new(inbound_rx),
            status: status_rx,
        })
    }
}

/// Resolve a `host:port` forwarder endpoint to one address.
fn resolve(endpoint: &str) -> Result<SocketAddr, LinkError> {
    endpoint
        .to_socket_addrs()
        .map_err(|_| LinkError::Endpoint(endpoint.to_string()))?
        .next()
        .ok_or_else(|| LinkError::Endpoint(endpoint.to_string()))
}

/// The task that owns the socket, the sessions and the sequence source.
struct Driver {
    socket: UdpSocket,
    forwarder: SocketAddr,
    sessions: HashMap<NodeId, Session>,
    node: AcquiredNode,
    inbound: mpsc::Sender<(NodeId, Vec<u8>)>,
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
        inbound: mpsc::Sender<(NodeId, Vec<u8>)>,
        status: watch::Sender<LinkStatus>,
    ) -> Self {
        let origin = Instant::now();
        let peers = if config.peers.is_empty() {
            vec![PeerConfig {
                node_id: node.state.peer_node_id,
                pair_key: node.state.pair_key.clone(),
            }]
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
                    if let Ok((len, _from)) = received {
                        self.handle_datagram(&buffer[..len]).await;
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
    async fn handle_datagram(&mut self, datagram: &[u8]) {
        let now = self.now_ms();
        let Ok(header) = crate::header::OuterHeader::decode(datagram) else {
            return;
        };
        let Some(session) = self.sessions.get_mut(&header.src) else {
            return;
        };
        if session.handle_datagram(datagram, now).is_err() {
            return;
        }
        let messages = session.take_messages();
        let peer = header.src;
        for message in messages {
            if self.inbound.send((peer, message)).await.is_err() {
                return;
            }
        }
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
