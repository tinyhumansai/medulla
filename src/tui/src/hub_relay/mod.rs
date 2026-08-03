//! Native orchestrator-hub wiring for the backend runtime.
//!
//! When the TUI resolves the backend runtime it spawns the hub in-process (on by
//! default) so a plain `medulla` run relays the hosted brain's delegated tasks to
//! linked hosts — no separate process, no core-js. The hub is a tokio task
//! aborted when its guard drops (TUI exit / panic). It starts with an empty
//! roster and you add workers live from the Workers tab (or pre-seed via
//! `MEDULLA_LINK_PEER` / `MEDULLA_HUB_WORKERS`); `MEDULLA_HUB=0` opts out.
//!
//! The same [`build_hub_config`] powers the `medulla hub` subcommand, so the
//! standalone and embedded hubs resolve identically.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use medulla::auth::Credentials;
use medulla::hub::{start_hub, HubConfig, HubLinkConfig, HubLinkPeer, HubSession, WorkerSpec};
use medulla_link::LinkHandle;

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
            name: w.label.unwrap_or_else(|| "medulla-worker".to_string()),
            description: format!("{} daemon", w.harness),
            harness: w.harness,
            // Remembered rosters carry no workspace: the local host's is
            // injected fresh each launch (see `build_hub_config_with_host`),
            // and a remote peer's is unknown here.
            workspace: None,
        })
        .collect()
}

/// Build the remote side of the hub from an enrolled link identity.
///
/// The current node-state schema carries the enrolled pair key. Every configured
/// peer with a valid wire id is routed through the already-open link handle;
/// absent such a row, the peer recorded in node state remains available.
fn link_from_config(env: &HashMap<String, String>, home: &Path) -> Option<HubLinkConfig> {
    let path = roster_path(home);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let config = medulla::config::load_config(path.to_str(), env, &cwd)
        .ok()?
        .config;
    let link = config
        .link
        .unwrap_or_else(|| medulla::config::default_link_config(env));
    link_from_resolved_config(&link, None)
}

/// Build hub link wiring from the same resolved config the TUI is displaying.
fn link_from_resolved_config(
    link: &medulla::config::LinkConfig,
    handle: Option<Arc<LinkHandle>>,
) -> Option<HubLinkConfig> {
    let state_dir = PathBuf::from(&link.state_dir);
    let state =
        medulla_link::keys::read_node_state(&medulla_link::keys::node_path(&state_dir)).ok()?;
    let enrolled: HashMap<_, _> = state
        .enrolled_peers()
        .into_iter()
        .map(|peer| (peer.node_id, peer.pair_key))
        .collect();
    let mut peers: Vec<HubLinkPeer> = link
        .peers
        .iter()
        .filter_map(|peer| {
            let node_id = medulla_link::keys::NodeId::from_hex(peer.node_id.as_deref()?)?;
            Some(HubLinkPeer {
                name: peer
                    .address
                    .clone()
                    .or_else(|| peer.name.clone())
                    .unwrap_or_else(|| peer.id.clone()),
                node_id,
                pair_key: enrolled.get(&node_id)?.clone(),
            })
        })
        .collect();
    for (node_id, pair_key) in enrolled {
        if peers.iter().any(|peer| peer.node_id == node_id) {
            continue;
        }
        peers.push(HubLinkPeer {
            name: node_id.to_string(),
            node_id,
            pair_key,
        });
    }
    Some(HubLinkConfig {
        state_dir,
        node_name: link
            .node_name
            .clone()
            .unwrap_or_else(|| state.node_id.to_string()),
        forwarder_endpoint: None,
        peers,
        handle,
    })
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
    local_addresses: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) -> medulla::hub::RosterSink {
    let path = roster_path(home);
    Arc::new(move |workers: &[medulla::hub::HubWorker]| {
        let local_addresses = local_addresses.lock().expect("host addresses").clone();
        let rows: Vec<medulla::config::HubWorkerConfig> = workers
            .iter()
            .filter(|w| !local_addresses.contains(&w.address))
            .map(|w| medulla::config::HubWorkerConfig {
                id: w.id.clone(),
                address: w.address.clone(),
                harness: w.harness.clone(),
                label: w.label.clone(),
                selected: w.selected,
                roles: w.roles.clone(),
            })
            .collect();
        if let Err(e) = medulla::config::persist_hub_workers(&path, &rows) {
            log(&format!("hub: could not save the worker roster ({e})"));
        }
    })
}

/// Parse pre-seeded worker specs from the environment:
/// `MEDULLA_HUB_WORKERS="id=addr,…"`, else a single `MEDULLA_LINK_PEER`
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
        name: "medulla-worker".to_string(),
        description: format!("{provider} daemon"),
        harness: provider.clone(),
        workspace: None,
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
        .get("MEDULLA_LINK_PEER")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return vec![spec(peer, peer)];
    }
    Vec::new()
}

/// The `[workflows]` policy remembered beside the roster.
///
/// Read straight from the file for the same reason the roster is: this module
/// runs before (and independently of) the TUI's own config load.
#[cfg(feature = "workflows")]
fn workflows_config(home: &Path) -> medulla::config::WorkflowsConfig {
    std::fs::read_to_string(roster_path(home))
        .ok()
        .and_then(|text| toml::from_str::<medulla::config::TuiConfig>(&text).ok())
        .map(|config| config.workflows)
        .unwrap_or_default()
}

/// The workflow store this hub advertises to the hosted orchestrator, or `None`
/// when it should advertise none.
///
/// This is the installation the cloud workflow plane exists for: the same
/// layered store the Workflows tab, the `medulla workflow` subcommand and the
/// MCP tools read, handed to the hub so a `medulla:workflow_request` is answered
/// from the one catalogue rather than a second view of it.
///
/// Withheld when `[workflows] enabled` is false. The bridge applies no policy of
/// its own — the refusal lives in the run path — so advertising graphs this host
/// would decline to run would only teach the orchestrator to delegate work that
/// bounces.
#[cfg(feature = "workflows")]
fn workflow_bridge(
    env: &HashMap<String, String>,
    home: &Path,
) -> Option<medulla::hub::WorkflowPlane> {
    let config = workflows_config(home);
    if !config.enabled {
        return None;
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let store = medulla::workflows::discover_store(env, &cwd);
    let model = (!config.default_model.is_empty()).then(|| config.default_model.clone());

    // ACP, not the legacy provider transport, for the same reason the TUI's own
    // copilot pane forces it: Medulla is an ACP *client*, and `session/new`'s
    // `mcpServers` is the only channel it has for handing the harness the
    // `medulla-workflows` tools. Without them an authoring turn is a chatbot
    // that cannot touch the graph it was asked to edit.
    let mut harness_env = env.clone();
    harness_env.insert(
        medulla::daemon::providers::HARNESS_PROTOCOL_ENV.to_string(),
        "acp".to_string(),
    );
    let copilot = std::sync::Arc::new(medulla::workflows::LocalCopilotDispatch::new(
        medulla::daemon::embedded::EmbeddedDaemonOptions {
            workspace: cwd.to_string_lossy().to_string(),
            default_provider: config.default_provider,
            model: model.clone(),
            env: harness_env,
            ..Default::default()
        },
    ));

    Some(std::sync::Arc::new(
        medulla::workflows::StoreWorkflowBridge::new(store)
            .with_copilot(
                copilot,
                medulla::workflows::LOCAL_WORKER_ADDRESS,
                config.default_provider,
                model,
            )
            // The same directory the copilot's harness runs in, which is what a
            // capability probe means by "where does this agent work". The core
            // reads it off the bridge, so leaving it unset would omit `cwd` from
            // every probe this host answers.
            .with_action_dir(cwd.to_string_lossy().to_string()),
    ))
}

/// Without the engine compiled in this host has no workflow store to advertise.
#[cfg(not(feature = "workflows"))]
fn workflow_bridge(
    _env: &HashMap<String, String>,
    _home: &Path,
) -> Option<medulla::hub::WorkflowPlane> {
    None
}

/// Whether the hub should run. **On by default** in the backend runtime — a
/// plain `medulla` login is enough, and workers are added live from the Workers
/// tab (or pre-seeded via `MEDULLA_LINK_PEER` / `MEDULLA_HUB_WORKERS`).
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

/// Build a [`HubConfig`] from the environment + the signed-in session, or `None`
/// when the hub should not run ([`hub_enabled`]) or nobody is signed in (the hub
/// needs a backend JWT for the Socket.IO handshake).
pub(crate) fn build_hub_config_with_log(
    env: &HashMap<String, String>,
    home: &Path,
    log: medulla::hub::HubLog,
    session: Option<&Credentials>,
) -> Option<HubConfig> {
    // No catalog: this builder is the no-host path used by probes and tests,
    // where nothing is advertised for a role.
    build_hub_config_with_host(env, home, log, None, session, Vec::new())
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
    session: Option<&Credentials>,
    agent_templates: Vec<medulla::runtime::AgentTemplate>,
) -> Option<HubConfig> {
    let link = link_from_config(env, home);
    build_hub_config_with_host_and_link(env, home, log, local, session, agent_templates, link)
}

/// Build a hub using link wiring already resolved by the embedding process.
fn build_hub_config_with_host_and_link(
    env: &HashMap<String, String>,
    home: &Path,
    log: medulla::hub::HubLog,
    local: Option<LocalDispatch>,
    session: Option<&Credentials>,
    agent_templates: Vec<medulla::runtime::AgentTemplate>,
    link: Option<HubLinkConfig>,
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
            {
                let local_addresses = dispatch.host_addresses.lock().expect("host addresses");
                workers.retain(|worker| !local_addresses.contains(&worker.address));
            }
            // Inserted in declaration order, so the primary leads and the
            // extras follow it the way they read in the config.
            for host in dispatch.hosts.iter().rev() {
                workers.retain(|worker| worker.address != host.address);
                workers.insert(0, host.clone());
            }
            (Some(dispatch.network.clone()), dispatch.hub_address.clone())
        }
        None => (None, String::new()),
    };
    // Passed in rather than read from disk: the embedded core holds the only
    // session, and a second lookup here could disagree with the runtime about
    // whether this process is signed in.
    let creds = session?.clone();
    let poll_ms = env
        .get("MEDULLA_HUB_POLL_MS")
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_POLL_MS);
    // The handle, not a copy of its contents: the sink reads it at save time,
    // which is the only moment that knows which hosts this device is binding.
    let persisted_local = local
        .as_ref()
        .map(|dispatch| dispatch.host_addresses.clone())
        .unwrap_or_default();
    Some(HubConfig {
        agent_templates,
        persist: Some(roster_sink(home, log.clone(), persisted_local)),
        log,
        backend_url: creds.base_url,
        jwt: creds.jwt,
        link,
        workers,
        poll: Duration::from_millis(poll_ms),
        local_network,
        local_address,
        subscription_strategy: subscription_strategy_from_config(home),
        workflows: workflow_bridge(env, home),
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
    session: Option<&Credentials>,
    startup: StartupConfig,
) -> Option<HubSession> {
    // The hub must never write to the terminal here: the TUI owns the alternate
    // screen, and ratatui only repaints the cells it manages, so a stray line
    // lands on top of the UI and is never cleared. Capturing them keeps the
    // screen intact and the diagnostics readable.
    let link = startup
        .link
        .as_ref()
        .and_then(|link| link_from_resolved_config(&link.config, Some(link.handle.clone())));
    let config = build_hub_config_with_host_and_link(
        env,
        home,
        logs.sink(),
        local,
        session,
        startup.agent_templates,
        link,
    )?;
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

/// Inputs already resolved by the TUI's layered configuration load.
pub(crate) struct StartupConfig {
    /// Agent roles shown by the same running TUI.
    pub agent_templates: Vec<medulla::runtime::AgentTemplate>,
    /// The one link handle shared by observation and dispatch.
    pub link: Option<ResolvedLink>,
}

/// A configured link and the handle that already owns its identity lock.
pub(crate) struct ResolvedLink {
    /// Effective `[link]` configuration after all layers are merged.
    pub config: medulla::config::LinkConfig,
    /// Link opened by the observation service.
    pub handle: Arc<LinkHandle>,
}
