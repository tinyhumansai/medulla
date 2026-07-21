//! Hub bootstrap: construct the tiny.place transport + sender-runner, connect the
//! Socket.IO harness client, and expose a live [`HubHandle`].
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

use crate::daemon::transport::SignalTransport;
use crate::tinyplace::{load_or_create_identity, resolve_endpoint};

use super::handle::HubHandle;
use super::roster::{HubWorker, SharedRoster};
use super::runner::TaskRunner;
use super::socket::connect_harness;

/// One worker the hub fronts on the backend roster.
#[derive(Debug, Clone)]
pub struct WorkerSpec {
    /// The `agentId` the backend targets (defaults to the tiny.place address).
    pub id: String,
    /// The worker's tiny.place address (base58 cryptoId or `@handle`).
    pub address: String,
    /// Display name for the roster entry.
    pub name: String,
    /// Free-text description / capability summary.
    pub description: String,
    /// The coding-agent harness the worker runs (`claude`/`codex`/`opencode`).
    pub harness: String,
}

/// Everything [`start_hub`] needs to bridge the backend to tiny.place workers.
#[derive(Debug, Clone)]
pub struct HubConfig {
    /// Backend Socket.IO base URL (e.g. `https://staging-api.tinyhumans.ai`).
    pub backend_url: String,
    /// JWT for the Socket.IO handshake (from `medulla login`).
    pub jwt: String,
    /// tiny.place identity directory (the hub's own wallet).
    pub identity_dir: PathBuf,
    /// The workers to advertise initially (may be empty; add more at runtime).
    pub workers: Vec<WorkerSpec>,
    /// How often the runner drains the encrypted inbox.
    pub poll: Duration,
    /// Per-task deadline for a worker's terminal reply.
    pub task_timeout: Duration,
}

/// A running hub: the live [`HubHandle`] plus the client/runner kept alive for
/// the session (dropping this disconnects and stops the pump).
pub struct HubSession {
    /// Live roster control (add/remove/list workers), re-registering on change.
    pub handle: HubHandle,
    _runner: Arc<TaskRunner>,
    _client: Client,
}

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
        base_url,
        signer: Some(signer.clone() as Arc<dyn Signer>),
        ..Default::default()
    });
    let transport = SignalTransport::new(client, &signer, &config.identity_dir);

    // Publish pre-keys so a worker can run X3DH against us (best-effort).
    if let Err(e) = transport.publish_keys(&signer).await {
        eprintln!("hub: pre-key publish skipped ({e})");
    }

    // The hub's own identity, captured before the transport moves into the
    // runner. Operators need it verbatim: a worker only accepts a task from a
    // peer it trusts, so this is what goes in the worker's
    // `TINYPLACE_OPENHUMAN_OWNER` / `acceptContacts` allowlist.
    let hub_address = transport.agent_id().to_string();
    let hub_public_key = transport.identity_key_base64();
    eprintln!("hub: identity {hub_address} (set as the worker's owner / allowlist it)");

    let runner = Arc::new(TaskRunner::start(Arc::new(transport), config.poll));

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
            })
            .collect(),
    ));

    eprintln!(
        "hub: connecting to {} ({} worker(s))",
        config.backend_url,
        config.workers.len()
    );
    let socket = connect_harness(
        &config.backend_url,
        &config.jwt,
        roster.clone(),
        runner.clone(),
        config.task_timeout,
    )
    .await?;
    eprintln!("hub: connected + registered — relaying tasks to tiny.place workers");

    let handle = HubHandle::new(roster, socket.clone(), hub_address, hub_public_key);
    Ok(HubSession {
        handle,
        _runner: runner,
        _client: socket,
    })
}

/// Standalone entry: start a hub session and hold until interrupted (Ctrl-C /
/// SIGINT, or the parent killing the process).
pub async fn run_hub(config: HubConfig) -> anyhow::Result<()> {
    let _session = start_hub(config).await?;
    tokio::signal::ctrl_c().await.ok();
    eprintln!("hub: shutting down");
    Ok(())
}
