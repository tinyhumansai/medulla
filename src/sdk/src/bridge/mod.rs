//! Message delivery bridges for local and remote agent communication.
//!
//! [`LocalBridge`] keeps every message in an in-memory bus owned by the current
//! process. [`TinyplaceBridge`] delegates to tiny.place's encrypted Signal
//! transport for peers that may live on another device. Consumers depend on the
//! shared [`Bridge`] contract, so routing code does not need to know where a
//! peer is running.

mod local;
mod tinyplace;
mod types;

#[cfg(test)]
mod tests;

pub use local::{LocalBridge, LocalBridgeNetwork};
pub use tinyplace::TinyplaceBridge;
pub use types::{Bridge, BridgeKind, BridgeTransport};
