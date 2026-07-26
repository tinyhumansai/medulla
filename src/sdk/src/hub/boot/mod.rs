//! Hub bootstrap: construct the remote tiny.place bridge + sender-runner,
//! connect the Socket.IO harness client, and expose a live [`HubHandle`].
//!
//! [`start_hub`] wires everything and returns a [`HubSession`] (holding the
//! handle plus the keep-alive client/runner) so an embedding host can manage the
//! roster at runtime; [`run_hub`] is the standalone wrapper that starts a session
//! and holds the process open until interrupted.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ::tinyplace::{Signer, TinyPlaceClient, TinyPlaceClientOptions};
use rust_socketio::asynchronous::Client;

use crate::bridge::{RoutingBridge, TinyplaceBridge};
use crate::daemon::transport::SignalTransport;
use crate::tinyplace::{load_or_create_identity, resolve_endpoint};

use super::handle::HubHandle;
use super::roster::{HubWorker, SharedRoster};
use super::runner::TaskRunner;
use super::socket::connect_harness;

/// The address a hub binds on the device-local bus when the caller names none.
pub const DEFAULT_LOCAL_HUB_ADDRESS: &str = "medulla-orchestrator";

/// Build the transport + runner, connect the harness client, and return a
/// [`HubSession`]. Errors only on fatal setup (bad identity, unreachable
/// backend); pre-key publish failures are non-fatal and logged.
pub async fn start_hub(config: HubConfig) -> anyhow::Result<HubSession> {
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();

    // tiny.place identity + client (mirrors the daemon's setup).
    let config_file = config.identity_dir.join("config.json");
    let (signer, tp_config) =
        load_or_create_identity(&config_file, &env).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let base_url = resolve_endpoint(&env, &tp_config);
    let signer = Arc::new(signer);
    let client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url: base_url.clone(),
        signer: Some(signer.clone() as Arc<dyn Signer>),
        ..Default::default()
    });
    let transport = SignalTransport::new(client.clone(), &signer, &config.identity_dir);

    // Inbound pairing. A worker that names this hub as its master requests
    // contact *here*, and until something reads that queue the relay keeps
    // refusing DMs in both directions — see [`super::pairing`] for why this
    // admits every request rather than an allowlist.
    let pairing = super::pairing::spawn_pairing(
        Arc::new(crate::contacts::ClientContacts::new(client)),
        config.log.clone(),
        config.poll,
    );

    // Publish pre-keys so a worker can run X3DH against us (best-effort).
    if let Err(e) = transport.publish_keys(&signer).await {
        (config.log)(&format!("hub: pre-key publish skipped ({e})"));
    }

    // The hub's own identity, captured before the transport moves into the
    // runner. Operators need it verbatim: a worker only accepts a task from a
    // peer it trusts, so this is what goes in the worker's
    // `TINYPLACE_OPENHUMAN_OWNER` / `acceptContacts` allowlist.
    let hub_address = transport.agent_id().to_string();
    let hub_public_key = transport.identity_key_base64();
    // The relay is named here, not just the backend: a contact request is
    // delivered on the relay, so a hub and a worker that disagree about it can
    // never reach each other however healthy both look.
    (config.log)(&format!(
        "hub: identity {hub_address} on {base_url} (set as the worker's owner / allowlist it)"
    ));

    // One transport, shared: the runner dispatches through it and the handle
    // opens contact edges through it. A second on the same wallet would be a
    // second writer to one Signal session store.
    let remote: Arc<dyn super::relay::Relay> = Arc::new(TinyplaceBridge::new(transport));
    // With a local bus attached the relay becomes a router: workers hosted in
    // this process are reached in memory and never touch the relay, while remote
    // ones fall through unchanged. Binding is fatal rather than best-effort —
    // a failure means something else already holds this address, and silently
    // continuing remote-only would look exactly like a host that never answers.
    let relay: Arc<dyn super::relay::Relay> = match &config.local_network {
        Some(network) => {
            let address = match config.local_address.trim() {
                "" => DEFAULT_LOCAL_HUB_ADDRESS,
                value => value,
            };
            let local = network.bind(address).map_err(|e| {
                anyhow::anyhow!("hub: could not bind local address {address} ({e})")
            })?;
            (config.log)(&format!("hub: also hosting on the local bus as {address}"));
            Arc::new(RoutingBridge::new(local, remote))
        }
        None => remote,
    };
    // One activity log, shared by the pump that observes frames and the socket
    // that dispatches them — the Agents view reads what both write.
    let activity = super::ActivityLog::new();
    let runner = Arc::new(TaskRunner::start_with_log_and_activity(
        relay.clone(),
        config.poll,
        config.log.clone(),
        activity.clone(),
    ));

    // The shared roster the socket advertises and the handle mutates.
    let roster: SharedRoster = Arc::new(Mutex::new(
        config
            .workers
            .iter()
            .map(|w| HubWorker {
                id: w.id.clone(),
                address: w.address.clone(),
                harness: w.harness.clone(),
                label: (w.name != "tinyplace-worker").then(|| w.name.clone()),
                selected: false,
                workspace: w.workspace.clone(),
            })
            .collect(),
    ));
    let subscription_strategy = Arc::new(Mutex::new(config.subscription_strategy));

    (config.log)(&format!(
        "hub: connecting to {} ({} worker(s))",
        config.backend_url,
        config.workers.len()
    ));
    let socket = connect_harness(
        &config.backend_url,
        &config.jwt,
        roster.clone(),
        runner.clone(),
        subscription_strategy.clone(),
        config.log.clone(),
        Some(activity.clone()),
    )
    .await?;
    (config.log)("hub: connected + registered — relaying tasks to tiny.place workers");

    let handle = HubHandle::new(super::handle::HandleWiring {
        roster,
        socket: socket.clone(),
        address: hub_address,
        public_key: hub_public_key,
        relay,
        runner: runner.clone(),
        log: config.log.clone(),
        persist: config.persist.clone(),
        activity,
        subscription_strategy,
    });
    Ok(HubSession {
        handle,
        _runner: runner,
        _client: socket,
        _pairing: pairing,
    })
}

/// Standalone entry: start a hub session and hold until interrupted (Ctrl-C /
/// SIGINT, or the parent killing the process).
pub async fn run_hub(config: HubConfig) -> anyhow::Result<()> {
    let log = config.log.clone();
    let _session = start_hub(config).await?;
    tokio::signal::ctrl_c().await.ok();
    log("hub: shutting down");
    Ok(())
}

mod types;
pub use types::HubConfig;
pub use types::HubSession;
pub use types::WorkerSpec;
