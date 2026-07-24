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
/// Default per-task deadline when `MEDULLA_HUB_TASK_TIMEOUT_S` is unset.
///
/// Must stay BELOW the backend's own `DEFAULT_TASK_TIMEOUT_MS` (300s): if the
/// hub outlives it, the backend gives up first and reports a blind
/// "subagent task timeout" instead of the hub's real error.
const DEFAULT_TASK_TIMEOUT_S: u64 = 240;

/// Hard ceiling for `MEDULLA_HUB_TASK_TIMEOUT_S`. The hub MUST expire before the
/// backend's 300s deadline so its real worker error wins the race; a configured
/// value of 300+ (or 0) is rejected in favour of this cap.
const MAX_TASK_TIMEOUT_S: u64 = 290;

/// Resolve the per-task timeout from `MEDULLA_HUB_TASK_TIMEOUT_S`, clamping to
/// `[1, MAX_TASK_TIMEOUT_S]`. An unparseable, zero, or above-ceiling value falls
/// back to a safe bound (default when unset/garbage, the cap when too large) so
/// the hub can never be configured to outlive the backend.
fn resolve_timeout_s(env: &HashMap<String, String>, log: &medulla::hub::HubLog) -> u64 {
    match env
        .get("MEDULLA_HUB_TASK_TIMEOUT_S")
        .and_then(|s| s.trim().parse::<u64>().ok())
    {
        None => DEFAULT_TASK_TIMEOUT_S,
        Some(0) => DEFAULT_TASK_TIMEOUT_S,
        Some(v) if v > MAX_TASK_TIMEOUT_S => {
            log(&format!(
                "hub: MEDULLA_HUB_TASK_TIMEOUT_S={v} exceeds the {MAX_TASK_TIMEOUT_S}s ceiling (must expire before the backend's 300s) — capping at {MAX_TASK_TIMEOUT_S}s"
            ));
            MAX_TASK_TIMEOUT_S
        }
        Some(v) => v,
    }
}

/// The shared slot a [`BackendRuntime`](medulla::runtime::backend::BackendRuntime)
/// reads for its live worker roster; filled once the hub connects.
pub(crate) type HubSlot = Arc<Mutex<Option<HubHandle>>>;

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

/// A sink that writes roster changes back to the config file.
///
/// Best-effort and narrated: failing to save a roster must not take the hub down
/// with it, but a silent failure would leave the operator re-adding the same
/// worker every launch with no idea why.
fn roster_sink(home: &Path, log: medulla::hub::HubLog) -> medulla::hub::RosterSink {
    let path = roster_path(home);
    Arc::new(move |workers: &[medulla::hub::HubWorker]| {
        let rows: Vec<medulla::config::HubWorkerConfig> = workers
            .iter()
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
    if !hub_enabled(env) {
        return None;
    }
    // Environment first: an explicitly exported roster is a deliberate override
    // for this run, and should not be quietly merged with a remembered one.
    let mut workers = workers_from_env(env);
    if workers.is_empty() {
        workers = workers_from_config(home);
    }
    let creds = medulla::auth::CredentialStore::at_home(home).load_or_legacy()?;
    let identity_dir = env
        .get("MEDULLA_HUB_IDENTITY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("tinyplace-hub"));
    let poll_ms = env
        .get("MEDULLA_HUB_POLL_MS")
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_POLL_MS);
    let timeout_s = resolve_timeout_s(env, &log);
    Some(HubConfig {
        persist: Some(roster_sink(home, log.clone())),
        log,
        backend_url: creds.base_url,
        jwt: creds.jwt,
        identity_dir,
        workers,
        poll: Duration::from_millis(poll_ms),
        task_timeout: Duration::from_secs(timeout_s),
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
) -> Option<HubSession> {
    // The hub must never write to the terminal here: the TUI owns the alternate
    // screen, and ratatui only repaints the cells it manages, so a stray line
    // lands on top of the UI and is never cleared. Capturing them keeps the
    // screen intact and the diagnostics readable.
    let config = build_hub_config_with_log(env, home, logs.sink())?;
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
