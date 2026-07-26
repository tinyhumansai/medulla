//! The live control handle over the hub's worker roster.
//!
//! Split from [`roster`](super::roster) because every mutation here is coupled to
//! the live Socket.IO uplink — it re-emits `medulla:register_agents` so the
//! backend's roster tracks a runtime add/remove — whereas `roster` holds the
//! pure, offline-testable roster data and address resolution. This driver is
//! exercised by the live staging E2E rather than unit tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::relay::Relay;
use super::roster::{register_payload, remove_conflicting, HubWorker, SharedRoster};
use rust_socketio::asynchronous::Client;

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

/// Cache a probe result only while the roster id still names the probed peer.
///
/// The roster lock stays held through the cache insert. This makes the check
/// atomic with add/remove operations, which mutate the roster before clearing
/// cached details for the affected ids.
pub(super) fn cache_system_info_if_current(
    roster: &SharedRoster,
    system_info: &Mutex<HashMap<String, crate::tinyplace::WorkerSystemInfo>>,
    id: &str,
    address: &str,
    info: crate::tinyplace::WorkerSystemInfo,
) -> bool {
    let workers = roster.lock().expect("roster lock");
    if !workers
        .iter()
        .any(|worker| worker.id == id && worker.address == address)
    {
        return false;
    }
    system_info
        .lock()
        .expect("system info lock")
        .insert(id.to_string(), info);
    true
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
            runner: wiring.runner,
            system_info: Arc::new(Mutex::new(HashMap::new())),
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

    /// Latest captured system details for a worker, if it has been refreshed.
    pub fn system_info(&self, id: &str) -> Option<crate::tinyplace::WorkerSystemInfo> {
        self.system_info
            .lock()
            .expect("system info lock")
            .get(id)
            .cloned()
    }

    /// Refresh and cache one worker's CPU, RAM, and IP details.
    pub async fn refresh_system_info(&self, id: &str) -> anyhow::Result<()> {
        let worker = self
            .list()
            .into_iter()
            .find(|worker| worker.id == id)
            .ok_or_else(|| anyhow::anyhow!("no worker {id} to refresh"))?;
        let info = self
            .runner
            .system_info(&worker.address)
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        if !cache_system_info_if_current(&self.roster, &self.system_info, id, &worker.address, info)
        {
            anyhow::bail!("worker {id} changed while details were refreshed");
        }
        Ok(())
    }

    /// Choose the current default worker from cached capacity details.
    pub fn apply_strategy(&self, strategy: crate::runtime::RoutingStrategy) -> anyhow::Result<()> {
        use crate::runtime::RoutingStrategy;

        if strategy == RoutingStrategy::Manual {
            return Ok(());
        }
        let roster = self.list();
        let details = self.system_info.lock().expect("system info lock");
        let selected =
            super::roster::worker_for_strategy(&roster, &details, strategy).ok_or_else(|| {
                anyhow::anyhow!("refresh worker details before applying an automatic strategy")
            })?;
        drop(details);
        self.select(&selected);
        Ok(())
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
        // Address-shape validation exists to catch a mis-paste of a tiny.place
        // cryptoId, which is otherwise silent. A device-local host is exempt:
        // its address is a name this process bound, so "does it exist" is
        // answerable exactly, and demanding base58 of it would make the host on
        // this very machine the one worker the roster refuses.
        if !is_plausible_address(&worker.address)
            && !self.relay.is_device_local(&worker.address).await
        {
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
        let replaced_ids = {
            let mut r = self.roster.lock().expect("roster lock");
            let replaced_ids = remove_conflicting(&mut r, &worker);
            // Give it an id the orchestrator can actually reproduce. Done after
            // conflict removal so a re-add reuses the freed name rather than
            // colliding with the entry it is replacing.
            if worker.id.trim().is_empty() || worker.id == worker.address {
                let taken: Vec<String> = r.iter().map(|w| w.id.clone()).collect();
                worker.id =
                    super::roster::worker_id(worker.label.as_deref(), &worker.harness, &taken);
            }
            r.push(worker);
            replaced_ids
        };
        if !replaced_ids.is_empty() {
            let mut details = self.system_info.lock().expect("system info lock");
            for id in replaced_ids {
                details.remove(&id);
            }
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
            self.system_info
                .lock()
                .expect("system info lock")
                .remove(id);
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

mod types;
pub(super) use types::HandleWiring;
pub use types::HubHandle;
