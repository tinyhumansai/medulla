//! The live control handle over the hub's worker roster.
//!
//! Split from [`roster`](super::roster) because every mutation here is coupled to
//! the live Socket.IO uplink — it re-emits `medulla:register_agents` so the
//! backend's roster tracks a runtime add/remove — whereas `roster` holds the
//! pure, offline-testable roster data and address resolution. This driver is
//! exercised by the live staging E2E rather than unit tests.

use super::roster::{register_payload, HubWorker, SharedRoster};
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
}

impl HubHandle {
    /// Build a handle over `roster`, re-registering through `socket`. `address`
    /// and `public_key` are the hub's *own* tiny.place identity — surfaced to the
    /// operator because every worker must trust it (as its owner / allowlisted
    /// peer) before it will accept a task.
    pub(super) fn new(
        roster: SharedRoster,
        socket: Client,
        address: String,
        public_key: String,
    ) -> Self {
        HubHandle {
            roster,
            socket,
            address,
            public_key,
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

    /// Add (or replace, by id) a worker and re-register.
    pub async fn add(&self, worker: HubWorker) -> anyhow::Result<()> {
        {
            let mut r = self.roster.lock().expect("roster lock");
            r.retain(|w| w.id != worker.id);
            r.push(worker);
        }
        self.reregister().await
    }

    /// Remove a worker by id and re-register.
    pub async fn remove(&self, id: &str) -> anyhow::Result<()> {
        {
            self.roster
                .lock()
                .expect("roster lock")
                .retain(|w| w.id != id);
        }
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
        self.reregister().await
    }

    /// Mark a worker as the selected default (local display state only).
    pub fn select(&self, id: &str) {
        let mut r = self.roster.lock().expect("roster lock");
        for w in r.iter_mut() {
            w.selected = w.id == id;
        }
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
