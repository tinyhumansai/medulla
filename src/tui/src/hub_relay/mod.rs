//! Native orchestrator-hub wiring for the backend runtime.
//!
//! When the TUI resolves the backend runtime it spawns the hub in-process (on by
//! default) so a plain `medulla` run relays the hosted brain's delegated tasks to
//! tiny.place workers — no separate process, no core-js. The hub is a tokio task
//! aborted when its guard drops (TUI exit / panic). It starts with an empty
//! roster and you add workers live from the Workers tab (or pre-seed via
//! `MEDULLA_TINYPLACE_PEER` / `MEDULLA_HUB_WORKERS`); `MEDULLA_HUB=0` opts out.
//!
//! The same [`build_hub_config`] powers the `medulla hub` subcommand, so the
//! standalone and embedded hubs resolve identically.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use medulla::hub::{start_hub, HubConfig, HubHandle, HubSession, WorkerSpec};

/// Default inbox poll interval when `MEDULLA_HUB_POLL_MS` is unset.
const DEFAULT_POLL_MS: u64 = 1500;

/// The config file the roster is remembered in.
///
/// Home-derived to match the rest of this module (identity dir, credentials).
/// `--config` is deliberately not consulted: the hub already resolves entirely
/// from env + home, and honouring it here alone would mean the roster and the
/// identity could come from different places.
fn roster_path(home: &Path) -> PathBuf {
    home.join("config.toml")
}

/// Workers remembered from a previous run.
///
/// Read straight from the file rather than a `LoadedConfig`, because this module
/// is reached before (and independently of) the TUI's config load, and because
/// the file it writes is the file it must read back.
fn workers_from_config(home: &Path) -> Vec<WorkerSpec> {
    let Ok(text) = std::fs::read_to_string(roster_path(home)) else {
        return Vec::new();
    };
    let Ok(config) = toml::from_str::<medulla::config::TuiConfig>(&text) else {
        return Vec::new();
    };
    config
        .hub
        .workers
        .into_iter()
        .map(|w| WorkerSpec {
            id: w.id,
            address: w.address,
            name: w.label.unwrap_or_else(|| "tinyplace-worker".to_string()),
            description: format!("{} daemon", w.harness),
            harness: w.harness,
        })
        .collect()
}

/// Subscription routing remembered beside the roster.
fn subscription_strategy_from_config(home: &Path) -> medulla::runtime::SubscriptionRoutingStrategy {
    let Ok(text) = std::fs::read_to_string(roster_path(home)) else {
        return medulla::runtime::SubscriptionRoutingStrategy::Manual;
    };
    toml::from_str::<medulla::config::TuiConfig>(&text)
        .ok()
        .and_then(|config| config.subscription_routing_strategy)
        .unwrap_or(medulla::runtime::SubscriptionRoutingStrategy::Manual)
}

/// A sink that writes roster changes back to the config file.
///
/// Best-effort and narrated: failing to save a roster must not take the hub down
/// with it, but a silent failure would leave the operator re-adding the same
/// worker every launch with no idea why.
/// `local_address` names the device-local host, which is deliberately *not*
/// saved: it is derived from `[host]` on every launch, so remembering it would
/// outlive the setting that produced it. A roster that kept it would, on the
/// next run with hosting off, advertise a worker whose address nothing binds —
/// and the router, finding no local endpoint, would send its tasks over
/// tiny.place to a name no relay can resolve.
fn roster_sink(
    home: &Path,
    log: medulla::hub::HubLog,
    local_address: String,
) -> medulla::hub::RosterSink {
    let path = roster_path(home);
    Arc::new(move |workers: &[medulla::hub::HubWorker]| {
        let rows: Vec<medulla::config::HubWorkerConfig> = workers
            .iter()
            .filter(|w| w.address != local_address)
            .map(|w| medulla::config::HubWorkerConfig {
                id: w.id.clone(),
                address: w.address.clone(),
                harness: w.harness.clone(),
                label: w.label.clone(),
                selected: w.selected,
            })
            .collect();
        if let Err(e) = medulla::config::persist_hub_workers(&path, &rows) {
            log(&format!("hub: could not save the worker roster ({e})"));
        }
    })
}

/// Parse pre-seeded worker specs from the environment:
/// `MEDULLA_HUB_WORKERS="id=addr,…"`, else a single `MEDULLA_TINYPLACE_PEER`
/// (id == address). Empty is fine — the hub starts with an empty roster and
/// workers are added live from the Workers tab.
fn workers_from_env(env: &HashMap<String, String>) -> Vec<WorkerSpec> {
    let provider = env
        .get("MEDULLA_WORKER_PROVIDER")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "claude".to_string());
    let spec = |id: &str, addr: &str| WorkerSpec {
        id: id.to_string(),
        address: addr.to_string(),
        name: "tinyplace-worker".to_string(),
        description: format!("{provider} daemon"),
        harness: provider.clone(),
    };
    if let Some(list) = env
        .get("MEDULLA_HUB_WORKERS")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|pair| {
                let (id, addr) = pair.split_once('=').unwrap_or((pair, pair));
                spec(id.trim(), addr.trim())
            })
            .collect();
    }
    if let Some(peer) = env
        .get("MEDULLA_TINYPLACE_PEER")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return vec![spec(peer, peer)];
    }
    Vec::new()
}

/// Whether the hub should run. **On by default** in the backend runtime — a
/// plain `medulla` login is enough, and workers are added live from the Workers
/// tab (or pre-seeded via `MEDULLA_TINYPLACE_PEER` / `MEDULLA_HUB_WORKERS`).
/// `MEDULLA_HUB=0`/`false` is the explicit kill-switch; `MEDULLA_HUB=1`/`true`
/// is the (redundant) explicit opt-in.
fn hub_enabled(env: &HashMap<String, String>) -> bool {
    match env
        .get("MEDULLA_HUB")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        Some(v) => medulla::home::is_truthy(v),
        None => true,
    }
}

/// Build a [`HubConfig`] from the environment + saved credentials, or `None`
/// when the hub should not run ([`hub_enabled`]) or the user is not logged in
/// (the hub needs a backend JWT for the Socket.IO handshake).
pub(crate) fn build_hub_config_with_log(
    env: &HashMap<String, String>,
    home: &Path,
    log: medulla::hub::HubLog,
) -> Option<HubConfig> {
    build_hub_config_with_host(env, home, log, None)
}

/// Like [`build_hub_config_with_log`], additionally dispatching over `local` —
/// the device-local bus a host in this same process is bound to.
///
/// `local` carries the bus and, when a host is running on it, the roster entry
/// naming that host. The entry is prepended rather than appended so the machine
/// the operator is sitting at leads the list, and it replaces any remembered
/// entry with the same address so restarting never accumulates duplicates of
/// itself.
pub(crate) fn build_hub_config_with_host(
    env: &HashMap<String, String>,
    home: &Path,
    log: medulla::hub::HubLog,
    local: Option<LocalDispatch>,
) -> Option<HubConfig> {
    if !hub_enabled(env) {
        return None;
    }
    // Environment first: an explicitly exported roster is a deliberate override
    // for this run, and should not be quietly merged with a remembered one.
    let mut workers = workers_from_env(env);
    if workers.is_empty() {
        workers = workers_from_config(home);
    }
    // The device-local host is injected fresh, never inherited. Remembered
    // entries at its address are dropped first — including when no host is
    // running now, which is the case that mattered: a roster saved while
    // hosting was on would otherwise keep advertising `this-device` after
    // `MEDULLA_HOST=0`, and its tasks would be routed to a relay that has never
    // heard the name.
    let (local_network, local_address) = match &local {
        Some(dispatch) => {
            workers.retain(|worker| worker.address != dispatch.host_address);
            if let Some(host) = &dispatch.host {
                workers.retain(|worker| worker.address != host.address);
                workers.insert(0, host.clone());
            }
            (Some(dispatch.network.clone()), dispatch.hub_address.clone())
        }
        None => (None, String::new()),
    };
    let creds = medulla::auth::CredentialStore::at_home(home).load_or_legacy()?;
    let identity_dir = env
        .get("MEDULLA_HUB_IDENTITY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("tinyplace-hub"));
    let poll_ms = env
        .get("MEDULLA_HUB_POLL_MS")
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_POLL_MS);
    let persisted_local = local
        .as_ref()
        .map(|dispatch| dispatch.host_address.clone())
        .unwrap_or_default();
    Some(HubConfig {
        persist: Some(roster_sink(home, log.clone(), persisted_local)),
        log,
        backend_url: creds.base_url,
        jwt: creds.jwt,
        identity_dir,
        workers,
        poll: Duration::from_millis(poll_ms),
        local_network,
        local_address,
        subscription_strategy: subscription_strategy_from_config(home),
    })
}

/// Start the hub, fill `slot` with its live handle, and return the running
/// session (dropping it disconnects). Returns `None` when the hub is disabled
/// (`MEDULLA_HUB=0`), not logged in, or fails to connect — the TUI runs fine
/// either way.
pub(crate) async fn start(
    env: &HashMap<String, String>,
    home: &Path,
    slot: HubSlot,
    logs: medulla_tui::log::LogBuffer,
    local: Option<LocalDispatch>,
) -> Option<HubSession> {
    // The hub must never write to the terminal here: the TUI owns the alternate
    // screen, and ratatui only repaints the cells it manages, so a stray line
    // lands on top of the UI and is never cleared. Capturing them keeps the
    // screen intact and the diagnostics readable.
    let config = build_hub_config_with_host(env, home, logs.sink(), local)?;
    match start_hub(config).await {
        Ok(session) => {
            *slot.lock().expect("hub slot") = Some(session.handle.clone());
            Some(session)
        }
        Err(e) => {
            logs.push(format!("hub: failed to start ({e})"));
            None
        }
    }
}

#[cfg(test)]
mod tests;

mod types;
pub(crate) use types::{HubSlot, LocalDispatch};
