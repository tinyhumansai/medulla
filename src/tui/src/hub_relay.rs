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
fn resolve_timeout_s(env: &HashMap<String, String>) -> u64 {
    match env
        .get("MEDULLA_HUB_TASK_TIMEOUT_S")
        .and_then(|s| s.trim().parse::<u64>().ok())
    {
        None => DEFAULT_TASK_TIMEOUT_S,
        Some(0) => DEFAULT_TASK_TIMEOUT_S,
        Some(v) if v > MAX_TASK_TIMEOUT_S => {
            eprintln!(
                "hub: MEDULLA_HUB_TASK_TIMEOUT_S={v} exceeds the {MAX_TASK_TIMEOUT_S}s ceiling (must expire before the backend's 300s) — capping at {MAX_TASK_TIMEOUT_S}s"
            );
            MAX_TASK_TIMEOUT_S
        }
        Some(v) => v,
    }
}

/// The shared slot a [`BackendRuntime`](medulla::runtime::backend::BackendRuntime)
/// reads for its live worker roster; filled once the hub connects.
pub(crate) type HubSlot = Arc<Mutex<Option<HubHandle>>>;

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
pub(crate) fn build_hub_config(env: &HashMap<String, String>, home: &Path) -> Option<HubConfig> {
    if !hub_enabled(env) {
        return None;
    }
    let workers = workers_from_env(env);
    let creds = medulla::auth::CredentialStore::at_home(home).load_or_legacy()?;
    let identity_dir = env
        .get("MEDULLA_HUB_IDENTITY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("tinyplace-hub"));
    let poll_ms = env
        .get("MEDULLA_HUB_POLL_MS")
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_POLL_MS);
    let timeout_s = resolve_timeout_s(env);
    Some(HubConfig {
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
) -> Option<HubSession> {
    let config = build_hub_config(env, home)?;
    match start_hub(config).await {
        Ok(session) => {
            *slot.lock().expect("hub slot") = Some(session.handle.clone());
            Some(session)
        }
        Err(e) => {
            eprintln!("hub: failed to start ({e})");
            None
        }
    }
}

#[cfg(test)]
mod tests;
