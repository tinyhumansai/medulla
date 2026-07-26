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
//! the hub identity to work that queue from. A contact edge is not authority: work
//! is only ever dispatched to a roster entry the operator added and selected, so
//! accepting one grants a peer nothing beyond the ability to DM this identity.
//! Every acceptance is narrated by peer id, so a pairing nobody asked for shows up
//! in the log instead of happening silently.

use std::sync::Arc;

use tokio::task::JoinHandle;

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

/// A running inbound-pairing poll, stopped on drop.
pub(crate) struct PairingPoll(JoinHandle<()>);

impl Drop for PairingPoll {
    fn drop(&mut self) {
        self.0.abort();
    }
}
