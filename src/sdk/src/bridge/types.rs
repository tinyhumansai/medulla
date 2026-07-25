//! Shared bridge contract, kind discriminator, and runtime-selected transport.

use async_trait::async_trait;

use crate::daemon::transport::InboundMessage;

use super::{LocalBridge, TinyplaceBridge};

/// The two supported message-delivery scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeKind {
    /// Messages stay on this device and never use tiny.place.
    Local,
    /// Messages travel to remote peers through tiny.place.
    Tinyplace,
}

impl BridgeKind {
    /// Parse a configuration value into a bridge kind.
    ///
    /// The spelling is case-insensitive. Unknown values fail closed instead of
    /// silently selecting a remote transport.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "local" => Some(Self::Local),
            "tinyplace" | "tiny.place" => Some(Self::Tinyplace),
            _ => None,
        }
    }

    /// Stable configuration label for this bridge kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Tinyplace => "tinyplace",
        }
    }
}

/// Message operations shared by device-local and tiny.place bridges.
///
/// A bridge owns one address. Sending targets another address; draining
/// destructively returns messages addressed to the bridge. Contact operations
/// are meaningful for tiny.place and reduce to local endpoint discovery for the
/// local bridge.
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

    /// Whether `peer` is ready to receive messages.
    async fn contact_accepted(&self, peer: &str) -> bool;

    /// Reset transport-specific session state for `peer`.
    async fn reset_session(&self, peer: &str);
}

/// A runtime-selected bridge without dynamic dispatch.
#[derive(Clone)]
pub enum BridgeTransport {
    /// Device-local in-memory delivery.
    Local(LocalBridge),
    /// Remote encrypted delivery over tiny.place.
    Tinyplace(Box<TinyplaceBridge>),
}

impl BridgeTransport {
    /// The selected delivery scope.
    pub const fn kind(&self) -> BridgeKind {
        match self {
            Self::Local(_) => BridgeKind::Local,
            Self::Tinyplace(_) => BridgeKind::Tinyplace,
        }
    }

    /// This bridge endpoint's address.
    pub fn address(&self) -> &str {
        match self {
            Self::Local(bridge) => bridge.address(),
            Self::Tinyplace(bridge) => bridge.address(),
        }
    }
}

#[async_trait]
impl Bridge for BridgeTransport {
    async fn send(&self, to: &str, body: &str) -> Result<(), String> {
        match self {
            Self::Local(bridge) => bridge.send(to, body).await,
            Self::Tinyplace(bridge) => bridge.send(to, body).await,
        }
    }

    async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage> {
        match self {
            Self::Local(bridge) => bridge.drain_inbox(limit).await,
            Self::Tinyplace(bridge) => bridge.drain_inbox(limit).await,
        }
    }

    async fn request_contact(&self, peer: &str) -> Result<(), String> {
        match self {
            Self::Local(bridge) => bridge.request_contact(peer).await,
            Self::Tinyplace(bridge) => bridge.request_contact(peer).await,
        }
    }

    async fn resolve_handle(&self, name: &str) -> Option<String> {
        match self {
            Self::Local(bridge) => bridge.resolve_handle(name).await,
            Self::Tinyplace(bridge) => bridge.resolve_handle(name).await,
        }
    }

    async fn contact_accepted(&self, peer: &str) -> bool {
        match self {
            Self::Local(bridge) => bridge.contact_accepted(peer).await,
            Self::Tinyplace(bridge) => bridge.contact_accepted(peer).await,
        }
    }

    async fn reset_session(&self, peer: &str) {
        match self {
            Self::Local(bridge) => bridge.reset_session(peer).await,
            Self::Tinyplace(bridge) => bridge.reset_session(peer).await,
        }
    }
}
