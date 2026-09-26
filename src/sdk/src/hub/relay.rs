//! Compatibility name for the hub's shared local-or-remote bridge contract.
//!
//! New code should prefer [`crate::bridge::Bridge`]. The `Relay` name remains
//! exported so existing SDK embedders and hub tests do not break.

pub use crate::bridge::Bridge as Relay;
