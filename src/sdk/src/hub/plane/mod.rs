//! The workflow plane's contract with the Medulla orchestration backend.
//!
//! [`payloads`] is the Socket.IO wire shape and [`bridge`] the store-side trait
//! an embedding host installs. Both were re-exported from the embedded OpenHuman
//! core until it was removed; they are declared here now because this host is
//! the only one left that speaks them.
//!
//! The transport that carries them is `hub::workflows`, kept separate so the
//! contract can be read without the plumbing.

pub mod bridge;
pub mod payloads;

pub use bridge::WorkflowBridge;
