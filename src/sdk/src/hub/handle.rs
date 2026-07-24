//! The live control handle over the hub's worker roster.
//!
//! Split from [`roster`](super::roster) because every mutation here is coupled to
//! the live Socket.IO uplink — it re-emits `medulla:register_agents` so the
//! backend's roster tracks a runtime add/remove — whereas `roster` holds the
//! pure, offline-testable roster data and address resolution. This driver is
//! exercised by the live staging E2E rather than unit tests.

use std::sync::Arc;

use super::relay::Relay;
use super::roster::{register_payload, remove_conflicting, HubWorker, SharedRoster};
use rust_socketio::asynchronous::Client;

/// A live control handle over the hub's roster, held by the TUI. Mutations
/// re-register the roster with the backend so a newly-added worker becomes a
/// delegation target (and a removed one stops being one) without a restart.
#[derive(Clone)]
pub struct HubHandle {
    roster: SharedRoster,
    socket: Client,
    address: String,
    public_key: String,
    /// The encrypted transport, used to open a contact edge with a peer the
    /// moment it is added rather than at first dispatch.
    relay: Arc<dyn Relay>,
    /// Where roster mutations are narrated. An add that quietly does nothing is
    /// the hardest kind of failure to chase.
    log: super::types::HubLog,
    /// Where the roster is written so it outlives the process. `None` keeps the
    /// old behaviour: in memory only, gone at exit.
    persist: Option<super::types::RosterSink>,
    /// What the workers are doing, for the Agents view.
    activity: super::ActivityLog,
}

/// Whether `address` is a directory alias rather than a cryptoId.
pub(super) fn is_handle(address: &str) -> bool {
    address.trim_start().starts_with('@')
}

/// Whether `address` could plausibly be a tiny.place destination.
///
/// A cryptoId is a base58-encoded 32-byte key, so it is 32-64 characters from
/// the base58 alphabet — which excludes `0`, `O`, `I` and `l` precisely because
/// they are easy to confuse. A handle is anything after a leading `@`.
///
/// This exists because a mis-paste is silent otherwise: a stray `>` was accepted
/// as an address, registered as a worker, and had a contact request sent to it.
/// Nothing downstream can tell that from a real peer that never replies.
pub(super) fn is_plausible_address(address: &str) -> bool {
    let address = address.trim();
    if is_handle(address) {
        return address.trim_start_matches('@').chars().count() >= 2;
    }
    let len = address.chars().count();
    (32..=64).contains(&len)
        && address
            .chars()
            .all(|c| c.is_ascii_alphanumeric() && !matches!(c, '0' | 'O' | 'I' | 'l'))
}

/// Whether adding a worker at `address` should send a contact request.
///
/// Split out so the rule is testable without a live Socket.IO client, which the
/// rest of this handle needs.
pub(super) fn should_request_contact(address: &str, accepted: bool) -> bool {
    !address.trim().is_empty() && !accepted
}

/// Everything a [`HubHandle`] is built from.
///
/// A struct rather than a parameter list: the handle needs the roster, the
/// uplink, the hub's own identity and three side-channels, and eight positional
/// arguments is a place where two of the same type get silently transposed.
pub(super) struct HandleWiring {
    /// The shared roster this handle mutates.
    pub roster: SharedRoster,
    /// The uplink to re-register through.
    pub socket: Client,
    /// The hub's own tiny.place address — surfaced to the operator because every
    /// worker must trust it before it will accept a task.
    pub address: String,
    /// The hub's own identity public key.
    pub public_key: String,
    /// The encrypted transport, for opening contact edges.
    pub relay: Arc<dyn Relay>,
    /// Where roster mutations are narrated.
    pub log: super::types::HubLog,
    /// Where the roster is saved, when it is saved at all.
    pub persist: Option<super::types::RosterSink>,
    /// What the workers are doing, for the Agents view.
    pub activity: super::ActivityLog,
}

impl HubHandle {
    /// Build a handle from its wiring.
    pub(super) fn new(wiring: HandleWiring) -> Self {
        HubHandle {
            roster: wiring.roster,
            socket: wiring.socket,
            address: wiring.address,
            public_key: wiring.public_key,
            relay: wiring.relay,
            log: wiring.log,
            persist: wiring.persist,
            activity: wiring.activity,
        }
    }

    /// What this hub's workers are doing right now.
    pub fn activity(&self) -> super::ActivityLog {
        self.activity.clone()
    }

    /// Write the current roster through the persist sink, if one is attached.
    ///
    /// Called after every mutation rather than on exit: a hub that is killed —
    /// which is how a TUI usually ends — would otherwise save nothing, and the
    /// roster this exists to remember is exactly what the operator just typed.
    fn save(&self) {
        if let Some(persist) = &self.persist {
            persist(&self.list());
        }
    }

    /// The hub's own tiny.place address (base58 cryptoId). This is the value an
    /// operator sets as a worker's `TINYPLACE_OPENHUMAN_OWNER` / adds to its
    /// `acceptContacts` allowlist.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The hub's own Ed25519 identity public key, base64.
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// A snapshot of the current roster.
    pub fn list(&self) -> Vec<HubWorker> {
        self.roster.lock().expect("roster lock").clone()
    }

    /// Add (or replace, by id) a worker, open a contact edge, and re-register.
    ///
    /// The contact request is sent here rather than only at first dispatch.
    /// A worker cannot receive a DM until it has accepted one, and its operator
    /// approves that request on screen — so deferring it means adding a peer
    /// looks like nothing happened, and the approval only appears much later,
    /// attached to a task that is already waiting on it.
    ///
    /// Re-adding an address that is already present therefore re-sends the
    /// request when the edge is not yet accepted, which is the natural way to
    /// retry one the peer missed. Requesting an existing contact is harmless, so
    /// the accepted case simply does nothing.
    ///
    /// It also *replaces* rather than duplicates: an entry matching either the
    /// id or the address is dropped first, so one peer can never occupy two
    /// roster slots however it was named.
    pub async fn add(&self, mut worker: HubWorker) -> anyhow::Result<()> {
        if !is_plausible_address(&worker.address) {
            let given = worker.address.clone();
            (self.log)(&format!("hub: refused worker address {given:?}"));
            anyhow::bail!(
                "{given:?} is not a tiny.place address — expected a base58 cryptoId or an @handle"
            );
        }
        // A handle is a directory alias; contacts, pre-key bundles and DMs are
        // all keyed on the cryptoId behind it. Storing the alias would register
        // a peer that nothing can address.
        if is_handle(&worker.address) {
            match self.relay.resolve_handle(&worker.address).await {
                Some(crypto_id) => {
                    (self.log)(&format!("hub: resolved {} → {crypto_id}", worker.address));
                    if worker.id == worker.address {
                        worker.id = crypto_id.clone();
                    }
                    worker.label.get_or_insert_with(|| worker.address.clone());
                    worker.address = crypto_id;
                }
                None => {
                    let name = worker.address.clone();
                    (self.log)(&format!("hub: {name} is not in the directory"));
                    anyhow::bail!("{name} is not in the tiny.place directory");
                }
            }
        }
        let address = worker.address.clone();
        {
            let mut r = self.roster.lock().expect("roster lock");
            remove_conflicting(&mut r, &worker);
            // Give it an id the orchestrator can actually reproduce. Done after
            // conflict removal so a re-add reuses the freed name rather than
            // colliding with the entry it is replacing.
            if worker.id.trim().is_empty() || worker.id == worker.address {
                let taken: Vec<String> = r.iter().map(|w| w.id.clone()).collect();
                worker.id =
                    super::roster::worker_id(worker.label.as_deref(), &worker.harness, &taken);
            }
            r.push(worker);
        }
        let accepted = if address.is_empty() {
            false
        } else {
            self.relay.contact_accepted(&address).await
        };
        if should_request_contact(&address, accepted) {
            // Best-effort: a peer that is unreachable right now is still a valid
            // roster entry, and dispatch retries the handshake anyway.
            match self.relay.request_contact(&address).await {
                Ok(()) => (self.log)(&format!(
                    "hub: worker {address} added · contact requested, awaiting its approval"
                )),
                Err(err) => (self.log)(&format!(
                    "hub: worker {address} added · contact request FAILED: {err}"
                )),
            }
        } else {
            (self.log)(&format!("hub: worker {address} added · already a contact"));
        }
        self.save();
        self.reregister().await
    }

    /// Whether `address` has accepted this hub's contact request.
    ///
    /// Lets a caller tell "added but waiting on approval" from "ready", which
    /// otherwise look identical in the roster.
    pub async fn contact_accepted(&self, address: &str) -> bool {
        self.relay.contact_accepted(address).await
    }

    /// Remove a worker by id and re-register.
    ///
    /// Reports whether anything was actually removed: an operator chasing a
    /// worker that keeps answering needs to know the id never matched, and
    /// "worker X removed" for an id that was not in the roster says the
    /// opposite.
    pub async fn remove(&self, id: &str) -> anyhow::Result<()> {
        let removed = {
            let mut r = self.roster.lock().expect("roster lock");
            let before = r.len();
            r.retain(|w| w.id != id);
            before != r.len()
        };
        if removed {
            (self.log)(&format!("hub: worker {id} removed"));
        } else {
            (self.log)(&format!("hub: no worker {id} to remove"));
        }
        self.save();
        self.reregister().await
    }

    /// Set a worker's label (no re-register needed — labels are display-only,
    /// but we re-advertise so the backend roster's `name` stays in sync).
    pub async fn set_label(&self, id: &str, label: Option<String>) -> anyhow::Result<()> {
        {
            let mut r = self.roster.lock().expect("roster lock");
            if let Some(w) = r.iter_mut().find(|w| w.id == id) {
                w.label = label;
            }
        }
        self.save();
        self.reregister().await
    }

    /// Mark a worker as the selected default (local display state only).
    pub fn select(&self, id: &str) {
        {
            let mut r = self.roster.lock().expect("roster lock");
            for w in r.iter_mut() {
                w.selected = w.id == id;
            }
        }
        self.save();
    }

    /// Re-emit `medulla:register_agents` for the current roster.
    async fn reregister(&self) -> anyhow::Result<()> {
        let payload = register_payload(&self.list());
        self.socket
            .emit("medulla:register_agents", payload)
            .await
            .map_err(|e| anyhow::anyhow!("re-register failed: {e}"))
    }
}
