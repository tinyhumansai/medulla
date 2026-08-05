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
use super::roster::{addresses_of, register_payload, remove_conflicting, HubWorker, SharedRoster};
use rust_socketio::asynchronous::Client;

/// Whether `address` is a directory alias rather than a cryptoId.
pub(super) fn is_handle(address: &str) -> bool {
    address.trim_start().starts_with('@')
}

/// Whether `address` could plausibly be a link destination.
///
/// A cryptoId is a base58-encoded 32-byte key, so it is 32-64 characters from
/// the base58 alphabet — which excludes `0`, `O`, `I` and `l` precisely because
/// they are easy to confuse. A handle is anything after a leading `@`.
///
/// This exists because a mis-paste is silent otherwise: a stray `>` was accepted
/// as an address and registered as a worker. Nothing downstream can tell that
/// from a real peer that never replies.
#[cfg(test)]
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

/// Cache a probe result only while the roster id still names the probed peer.
///
/// The roster lock stays held through the cache insert. This makes the check
/// atomic with add/remove operations, which mutate the roster before clearing
/// cached details for the affected ids.
pub(super) fn cache_system_info_if_current(
    roster: &SharedRoster,
    system_info: &Mutex<HashMap<String, crate::protocol::WorkerSystemInfo>>,
    id: &str,
    address: &str,
    info: crate::protocol::WorkerSystemInfo,
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
    /// The worker screens this hub is holding, for the view that renders them.
    pub fn screens(&self) -> super::ScreenStore {
        self.runner.screens()
    }

    /// Ask `worker` to start streaming the screen of the session running
    /// `task_id`, and to send a full frame.
    ///
    /// Always a resync: the hub may hold a stale screen from an earlier
    /// subscription, and a delta against that would apply to the wrong base.
    pub async fn watch(&self, worker: &str, task_id: &str) -> Result<(), String> {
        let body =
            crate::protocol::encode_screen_message(&crate::protocol::ScreenMessage::Subscribe {
                task_id: task_id.to_string(),
                max_fps: 1,
                resync: true,
            });
        // Armed before the send, so a frame that beats this function's return
        // is still recognised as wanted.
        self.runner.screens().arm(worker, task_id);
        let sent = self.relay.send(worker, &body).await;
        // Narrated after the send, and only for what actually happened. Logged
        // before it, a send that failed still read as a watch that started —
        // which points every later question at the worker ("why did it never
        // stream?") when the request never left this process.
        match &sent {
            Ok(()) => (self.log)(&format!("hub: watching task {task_id} on {worker}")),
            Err(error) => {
                self.runner.screens().disarm(worker, task_id);
                (self.log)(&format!(
                    "hub: could not ask {worker} to stream {task_id} — {error}"
                ));
            }
        }
        sent
    }

    /// Ask `worker` to stop streaming `task_id`, and drop what we hold.
    ///
    /// Forgotten locally even when the request fails to send: a pane left
    /// showing a screen that will never update again reads as a hung worker
    /// rather than an ended subscription.
    pub async fn unwatch(&self, worker: &str, task_id: &str) -> Result<(), String> {
        let body =
            crate::protocol::encode_screen_message(&crate::protocol::ScreenMessage::Unsubscribe {
                task_id: task_id.to_string(),
            });
        let sent = self.relay.send(worker, &body).await;
        // Disarmed, not forgotten: looking away should not throw away a screen
        // that is already here. Looking back redraws it at once, with its age
        // in the title, while the fresh subscribe catches up.
        self.runner.screens().disarm(worker, task_id);
        sent
    }

    /// Ask `worker` to kill the harness serving `task_id`.
    ///
    /// The worker resolves the task against the authenticated sender before it
    /// touches a PTY, so this cannot be used to kill another controller's work.
    pub async fn kill(&self, worker: &str, task_id: &str) -> Result<(), String> {
        let (correlation_id, wire_task_id) = self
            .runner
            .kill_correlation_for(worker, task_id)
            .await
            .ok_or_else(|| {
                format!("task {task_id} is not running with termination support on {worker}")
            })?;
        let body = crate::protocol::encode_screen_message(&crate::protocol::ScreenMessage::Kill {
            task_id: wire_task_id,
            correlation_id,
        });
        (self.log)(&format!("hub: killing task {task_id} on {worker}"));
        self.relay.send(worker, &body).await
    }

    /// Build a handle from its wiring.
    pub(super) fn new(wiring: HandleWiring) -> Self {
        HubHandle {
            roster: wiring.roster,
            socket: wiring.socket,
            address: wiring.address,
            relay: wiring.relay,
            catalog: wiring.catalog,
            local_hosts: wiring.local_hosts,
            runner: wiring.runner,
            system_info: Arc::new(Mutex::new(HashMap::new())),
            log: wiring.log,
            persist: wiring.persist,
            activity: wiring.activity,
            subscription_strategy: wiring.subscription_strategy,
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

    /// The hub's own link node name. This is the value an operator sets as a
    /// worker's `MEDULLA_LINK_OWNER`, because a worker only accepts a task from
    /// the orchestrator it enrolled with.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// A snapshot of the current roster.
    pub fn list(&self) -> Vec<HubWorker> {
        self.roster.lock().expect("roster lock").clone()
    }

    /// The dispatcher behind this hub.
    ///
    /// For a caller that needs to *run* a task rather than manage the roster —
    /// the control plane, which serves the harnesses Medulla spawns. Handed out
    /// as the runner rather than as a
    /// [`HarnessDispatch`](crate::flow_engine::caps::HarnessDispatch) because
    /// that trait's only cancellation is all-or-nothing
    /// ([`abort_all`](super::TaskRunner::abort_all)), and a caller holding
    /// several concurrent dispatches on behalf of one harness needs to stop
    /// exactly one of them.
    pub fn task_runner(&self) -> Arc<super::TaskRunner> {
        self.runner.clone()
    }

    /// Latest captured system details for a worker, if it has been refreshed.
    pub fn system_info(&self, id: &str) -> Option<crate::protocol::WorkerSystemInfo> {
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

    /// Change how untargeted tasks choose a ready provider subscription.
    pub fn apply_subscription_strategy(
        &self,
        strategy: crate::runtime::SubscriptionRoutingStrategy,
    ) {
        *self
            .subscription_strategy
            .lock()
            .expect("subscription strategy lock") = strategy;
    }

    /// Add (or replace, by id) a worker and re-register.
    ///
    /// There is no handshake to open: enrollment already established the pair
    /// key, so an address this hub can resolve is an address it may send to. The
    /// enrollment check above is the whole admission decision.
    ///
    /// It *replaces* rather than duplicates: an entry matching either the
    /// id or the address is dropped first, so one peer can never occupy two
    /// roster slots however it was named.
    pub async fn add(&self, mut worker: HubWorker) -> anyhow::Result<()> {
        // Address-shape validation exists to catch a mis-paste of a link
        // cryptoId, which is otherwise silent. A device-local host is exempt:
        // its address is a name this process bound, so "does it exist" is
        // answerable exactly, and demanding base58 of it would make the host on
        // this very machine the one worker the roster refuses.
        let enrolled_address = self.relay.resolve_handle(&worker.address).await;
        let device_local = self.relay.is_device_local(&worker.address).await;
        if !device_local && enrolled_address.is_none() {
            let given = worker.address.clone();
            (self.log)(&format!("hub: refused worker address {given:?}"));
            anyhow::bail!("{given:?} is not an enrolled link peer");
        }
        // A handle is a directory alias; contacts, pre-key bundles and DMs are
        // all keyed on the cryptoId behind it. Storing the alias would register
        // a peer that nothing can address.
        if is_handle(&worker.address) {
            match enrolled_address {
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
                    anyhow::bail!("{name} is not in the peer roster");
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
        (self.log)(&format!("hub: worker {address} added"));
        self.save();
        self.reregister().await
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

    /// Replace the agent-template roles a worker is offered for.
    ///
    /// Unlike a label this is not display-only: roles ride the descriptor the
    /// hub advertises, so the re-register is what actually makes the
    /// orchestrator start routing role-matched subtasks here.
    /// Errors when no worker holds `id` — a host can be removed between the
    /// render the operator toggled against and this call, and reporting that as
    /// a successful role change would leave the UI claiming a roster state that
    /// never existed.
    pub async fn set_roles(&self, id: &str, roles: Vec<String>) -> anyhow::Result<()> {
        {
            let mut r = self.roster.lock().expect("roster lock");
            let Some(w) = r.iter_mut().find(|w| w.id == id) else {
                anyhow::bail!("no host {id} to set roles on");
            };
            w.roles = roles;
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
    ///
    /// Re-asks the relay who is up rather than reusing the last answer: this
    /// runs on every roster mutation, and presence expires on a TTL, so a
    /// worker that died since the last advertisement is caught here.
    async fn reregister(&self) -> anyhow::Result<()> {
        let workers = self.list();
        let online = self.relay.presence(&addresses_of(&workers)).await;
        // Read here rather than captured at build time: a host started since
        // this handle was made must register as `local`, not as a remote host
        // the hub happens to front.
        let payload = {
            let local_hosts = self.local_hosts.lock().expect("local hosts lock");
            register_payload(&workers, &online, &self.catalog, &local_hosts)
        };
        self.socket
            .emit("medulla:register_agents", payload)
            .await
            .map_err(|e| anyhow::anyhow!("re-register failed: {e}"))
    }
}

mod handoff;
mod types;
pub(super) use types::HandleWiring;
pub use types::HubHandle;
