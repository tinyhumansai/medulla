//! Shared bridge contract, kind discriminator, and runtime-selected transport.

use async_trait::async_trait;
use medulla_link::keys::NodeId;

use super::{LinkBridge, LocalBridge};

/// One reachable link peer: its bridge name and wire identifier.
#[derive(Debug, Clone)]
pub struct LinkPeer {
    /// The human-readable address used by bridge callers.
    pub name: String,
    /// The identifier carried by link packets.
    pub node_id: NodeId,
}

/// Configuration for wrapping an already-connected host link.
#[derive(Debug, Clone)]
pub struct LinkBridgeConfig {
    /// This endpoint's bridge address.
    pub node_name: String,
    /// Every remote peer this endpoint can address.
    pub peers: Vec<LinkPeer>,
}

/// One inbound message, addressed to this bridge.
///
/// The bridge's whole delivery vocabulary: who sent it and what they said. It
/// lives here rather than in a transport module because every bridge — the
/// in-memory bus, the host link, the routing composite and every test fake —
/// hands back the same shape.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    /// The sender's bridge address.
    pub from: String,
    /// The message body.
    pub text: String,
}

/// How reachable a peer looks, as the transport below sees it.
///
/// Mirrors [`medulla_link::Liveness`] (host-link protocol §6.2) without making
/// every `Bridge` implementation depend on the link crate. **Advisory**: the
/// link keeps retransmitting through all three states, so an `Offline` peer is
/// not a failed one and nothing above the bridge may treat it as terminal. Its
/// purpose is to *pause* the timeouts above the link (§6.3), never to fire them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BridgeLiveness {
    /// Heard from recently; clocks above the link run.
    Live,
    /// Silent for a while, still being retransmitted to.
    Degraded,
    /// Silent for a long while, still being retransmitted to.
    Offline,
}

/// The two supported message-delivery scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeKind {
    /// Messages stay on this device and never leave the process.
    Local,
    /// Messages travel to remote hosts over the medulla host link.
    Link,
}

impl BridgeKind {
    /// Parse a configuration value into a bridge kind.
    ///
    /// The spelling is case-insensitive. Unknown values fail closed instead of
    /// silently selecting a remote transport.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "local" => Some(Self::Local),
            "link" => Some(Self::Link),
            _ => None,
        }
    }

    /// Stable configuration label for this bridge kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Link => "link",
        }
    }
}

/// Message operations shared by device-local and remote bridges.
///
/// A bridge owns one address. Sending targets another address; draining
/// destructively returns messages addressed to the bridge. The contact
/// operations are vestigial under the host link — it needs no handshake before
/// the first byte — and reduce to local endpoint discovery for the local
/// bridge.
#[async_trait]
pub trait Bridge: Send + Sync {
    /// Send `body` to `to`.
    async fn send(&self, to: &str, body: &str) -> Result<(), String>;

    /// Destructively drain up to `limit` inbound messages.
    async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage>;

    /// Establish permission to communicate with `peer`.
    async fn request_contact(&self, peer: &str) -> Result<(), String>;

    /// Resolve a human-readable peer name to its bridge address.
    async fn resolve_handle(&self, _name: &str) -> Option<String> {
        None
    }

    /// Whether `address` is served by this device rather than a remote peer.
    ///
    /// Callers use this to skip machinery that only makes sense across a
    /// network: address-shape validation, contact edges, session resets. It
    /// defaults to `false` so a transport that has no device-local scope — every
    /// remote one — is never mistaken for having one.
    async fn is_device_local(&self, _address: &str) -> bool {
        false
    }

    /// Whether `peer` is ready to receive messages.
    async fn contact_accepted(&self, peer: &str) -> bool;

    /// Reset transport-specific session state for `peer`.
    async fn reset_session(&self, peer: &str);

    /// How reachable `peer` looks to the transport underneath.
    ///
    /// Defaulted to [`BridgeLiveness::Live`], because a transport with no notion
    /// of reachability — the in-memory bus, every test fake — is always
    /// reachable, and the callers that gate their clocks on this must behave
    /// exactly as they did before when the answer is "fine".
    async fn liveness(&self, _peer: &str) -> BridgeLiveness {
        BridgeLiveness::Live
    }

    /// Which of `addresses` are reachable right now.
    ///
    /// Returns one entry per address that could be resolved; an address absent
    /// from the map has no answer, which is not the same as being offline and
    /// callers must not treat it as such.
    ///
    /// Defaulted to "no opinion" — an empty map — so a bridge with no
    /// reachability signal is never mistaken for one reporting everybody
    /// offline. The link bridge derives it from [`liveness`](Bridge::liveness),
    /// which is the only reachability signal the host link has.
    async fn presence(&self, _addresses: &[String]) -> std::collections::HashMap<String, bool> {
        std::collections::HashMap::new()
    }

    /// Wait until there may be inbound mail, or `poll` elapses.
    ///
    /// Defaulted to a plain sleep, which is exactly what every pump did before
    /// there was anything to wait *on* — so a local or fake bridge needs no
    /// changes. The link bridge overrides it to also return the moment its pump
    /// delivers, which is what takes a pump's latency floor off its poll
    /// interval.
    async fn wait_for_inbox(&self, poll: std::time::Duration) {
        tokio::time::sleep(poll).await;
    }
}

/// A runtime-selected bridge without dynamic dispatch.
#[derive(Clone)]
pub enum BridgeTransport {
    /// Device-local in-memory delivery.
    Local(LocalBridge),
    /// Remote end-to-end encrypted delivery over the host link.
    Link(Box<LinkBridge>),
}

impl BridgeTransport {
    /// The selected delivery scope.
    pub const fn kind(&self) -> BridgeKind {
        match self {
            Self::Local(_) => BridgeKind::Local,
            Self::Link(_) => BridgeKind::Link,
        }
    }

    /// This bridge endpoint's address.
    pub fn address(&self) -> &str {
        match self {
            Self::Local(bridge) => bridge.address(),
            Self::Link(bridge) => bridge.address(),
        }
    }
}

#[async_trait]
impl Bridge for BridgeTransport {
    async fn send(&self, to: &str, body: &str) -> Result<(), String> {
        match self {
            Self::Local(bridge) => bridge.send(to, body).await,
            Self::Link(bridge) => bridge.send(to, body).await,
        }
    }

    async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage> {
        match self {
            Self::Local(bridge) => bridge.drain_inbox(limit).await,
            Self::Link(bridge) => bridge.drain_inbox(limit).await,
        }
    }

    async fn request_contact(&self, peer: &str) -> Result<(), String> {
        match self {
            Self::Local(bridge) => bridge.request_contact(peer).await,
            Self::Link(bridge) => bridge.request_contact(peer).await,
        }
    }

    async fn resolve_handle(&self, name: &str) -> Option<String> {
        match self {
            Self::Local(bridge) => bridge.resolve_handle(name).await,
            Self::Link(bridge) => bridge.resolve_handle(name).await,
        }
    }

    async fn is_device_local(&self, address: &str) -> bool {
        match self {
            Self::Local(bridge) => bridge.is_device_local(address).await,
            Self::Link(bridge) => bridge.is_device_local(address).await,
        }
    }

    async fn contact_accepted(&self, peer: &str) -> bool {
        match self {
            Self::Local(bridge) => bridge.contact_accepted(peer).await,
            Self::Link(bridge) => bridge.contact_accepted(peer).await,
        }
    }

    async fn reset_session(&self, peer: &str) {
        match self {
            Self::Local(bridge) => bridge.reset_session(peer).await,
            Self::Link(bridge) => bridge.reset_session(peer).await,
        }
    }

    async fn liveness(&self, peer: &str) -> BridgeLiveness {
        match self {
            Self::Local(bridge) => bridge.liveness(peer).await,
            Self::Link(bridge) => bridge.liveness(peer).await,
        }
    }

    async fn presence(&self, addresses: &[String]) -> std::collections::HashMap<String, bool> {
        match self {
            Self::Local(bridge) => bridge.presence(addresses).await,
            Self::Link(bridge) => bridge.presence(addresses).await,
        }
    }

    async fn wait_for_inbox(&self, poll: std::time::Duration) {
        match self {
            Self::Local(bridge) => bridge.wait_for_inbox(poll).await,
            Self::Link(bridge) => bridge.wait_for_inbox(poll).await,
        }
    }
}
