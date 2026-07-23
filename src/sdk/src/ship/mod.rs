//! Read-first GitHub pull-request visibility and explicit ship actions.
//!
//! [`ShipClient`] isolates `gh` subprocesses from UI code. Read probes degrade
//! into [`ShipState::GhUnavailable`]; write actions remain explicit method calls.

mod client;
mod parse;
mod types;

#[cfg(test)]
mod tests;

pub use client::ShipClient;
pub use types::{CheckState, PrSummary, ShipError, ShipState, WorkspaceShipReport};
