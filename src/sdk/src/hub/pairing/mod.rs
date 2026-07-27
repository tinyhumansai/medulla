//! Inbound pairing for the hub's own tiny.place identity.
//!
//! The hub dials workers, but pairing also happens the other way round: a worker
//! operator names their master in the daemon's Master tab, and that sends *this*
//! identity a contact request. Nothing read that queue before — the hub only ever
//! *sent* requests — so a worker-initiated pairing stayed `pending` forever. The
//! relay refuses a DM between non-contacts, so the symptom on the worker was that
//! the master it had just added never came up: the row stayed offline, messages to
//! it were refused, and the only route through was for the master's operator to
//! add the worker by address so the worker could accept *that* request instead.
//!
//! Admission here is [`AdmissionPolicy::All`], deliberately. The hub cannot know
//! the address of a worker that has not paired yet, so an allowlist would queue
//! exactly the request this exists to answer — and there is no operator screen on
//! the hub identity to work that queue from.
//!
//! What an accepted contact can then do is bounded on the other side, in
//! [`super::runner::pump`]: every inbound frame is matched against the address the
//! dispatch or probe it claims to answer was actually sent to, and dropped if the
//! sender is anyone else. That check is what makes open admission safe — the
//! shared inbox and predictable correlation ids would otherwise let any contact
//! settle another worker's task or answer a capability probe on its behalf.
//! Beyond that, a contact edge is not authority: work is only ever dispatched to a
//! roster entry the operator added and selected. Every acceptance is narrated by
//! peer id, so a pairing nobody asked for shows up in the log rather than
//! happening silently.

use std::sync::Arc;

use crate::contacts::{AdmissionPolicy, ContactDesk, ContactRelay};

use super::types::HubLog;

/// Build the desk that admits inbound pairing requests, narrating to `log`.
///
/// Split from [`spawn_pairing`] so the admission behaviour is testable against a
/// stand-in relay, without a live tiny.place client.
pub(super) fn pairing_desk(relay: Arc<dyn ContactRelay>, log: HubLog) -> ContactDesk {
    ContactDesk::new(relay, AdmissionPolicy::All, Vec::<String>::new()).with_log(log)
}

/// Poll `relay` for inbound pairing requests every `interval`, accepting them.
///
/// The returned guard aborts the poll when dropped, so a hub session that ends
/// does not leave a loop talking to the relay on a dead identity's behalf.
pub(super) fn spawn_pairing(
    relay: Arc<dyn ContactRelay>,
    log: HubLog,
    interval: std::time::Duration,
) -> PairingPoll {
    PairingPoll(pairing_desk(relay, log).spawn_poll(interval))
}

#[cfg(test)]
mod tests;

mod types;
pub(crate) use types::PairingPoll;
