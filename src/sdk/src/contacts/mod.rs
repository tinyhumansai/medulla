//! Incoming contact-request management for tiny.place peers.
//!
//! The relay refuses a DM between two agents that are not accepted contacts, so
//! accepting a contact request is the act that lets a peer dispatch work to this
//! machine's coding agents. That makes it a privilege grant, not plumbing.
//!
//! Every prior implementation in this lineage auto-accepted every request. This
//! module keeps that available ([`AdmissionPolicy::All`]) but defaults to
//! [`AdmissionPolicy::Manual`], surfacing a queue the operator works through —
//! which is what the Sessions tab renders.
//!
//! - [`types`] — the policy, request, and decision model.
//! - [`book`] — [`ContactBook`], the pending queue and policy evaluation (pure).
//! - [`desk`] — [`ContactDesk`], the book/relay/clock bundle a UI holds.
//! - [`service`] — the relay seam, the poll tick, and decision execution.

pub mod book;
pub mod desk;
pub mod service;
pub mod types;

#[cfg(test)]
mod tests;

pub use book::ContactBook;
pub use desk::{ContactDesk, PollHealth};
pub use service::{
    decide, poll_once, reconcile_contacts, spawn_contact_poll, ClientContacts, ContactRelay,
    IncomingRequest,
};
pub use types::{AdmissionPolicy, ContactDecision, ContactRequest, RequestState};
