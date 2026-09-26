//! Message delivery bridges for local and remote agent communication.
//!
//! [`LocalBridge`] keeps every message in an in-memory bus owned by the current
//! process. [`LinkBridge`] delegates to the medulla host link for peers that may
//! live on another device. Consumers depend on the shared [`Bridge`] contract,
//! so routing code does not need to know where a peer is running.
//!
//! [`RoutingBridge`] composes the two: one endpoint that keeps device-local
//! peers on the in-memory bus and sends everything else over the link. That is
//! what lets a single process act as both orchestrator and host.

mod link;
mod local;
mod routing;
mod types;

#[cfg(test)]
mod tests;

pub use link::LinkBridge;
pub use local::{LocalBridge, LocalBridgeNetwork};
pub use routing::RoutingBridge;
pub use types::{
    Bridge, BridgeKind, BridgeLiveness, BridgeTransport, InboundMessage, LinkBridgeConfig, LinkPeer,
};
