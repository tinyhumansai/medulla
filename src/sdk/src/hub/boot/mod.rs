//! Hub bootstrap: bring up the host link, build the bridge + sender-runner,
//! connect the Socket.IO harness client, and expose a live [`HubHandle`].
//!
//! [`start_hub`] wires everything and returns a [`HubSession`] (holding the
//! handle plus the keep-alive client/runner) so an embedding host can manage the
//! roster at runtime; [`run_hub`] is the standalone wrapper that starts a session
//! and holds the process open until interrupted.
//!
//! A hub without a link identity is not an error. It becomes a local-only
//! router: it still orchestrates the host in this process, and a remote address
//! reports plainly that there is no transport for it rather than being accepted
//! and dropped.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use medulla_link::{Link, LinkConfig, PeerConfig};
use rust_socketio::asynchronous::Client;

use crate::bridge::{LinkBridge, LinkBridgeConfig, RoutingBridge};

use super::handle::HubHandle;
use super::roster::{HubWorker, SharedRoster};
use super::runner::TaskRunner;
use super::socket::connect_harness;

/// The address a hub binds on the device-local bus when the caller names none.
pub const DEFAULT_LOCAL_HUB_ADDRESS: &str = "medulla-orchestrator";

/// Build the transport + runner, connect the harness client, and return a
/// [`HubSession`]. Errors only on fatal setup (unusable link identity,
/// unreachable backend).
pub async fn start_hub(config: HubConfig) -> anyhow::Result<HubSession> {
    // The remote half. `None` is the ordinary single-device case, not a failure.
    let remote = match &config.link {
        Some(link) => Some(connect_link(link, &config.log).await?),
        None => None,
    };
    let hub_address = remote
        .as_ref()
        .map(|bridge| bridge.address().to_string())
        .unwrap_or_else(|| config.local_address.clone());

    // With a local bus attached the relay becomes a router: workers hosted in
    // this process are reached in memory and never touch the link, while remote
    // ones fall through unchanged. Binding is fatal rather than best-effort —
    // a failure means something else already holds this address, and silently
    // continuing remote-only would look exactly like a host that never answers.
    let relay: Arc<dyn super::relay::Relay> = match (&config.local_network, remote) {
        (Some(network), remote) => {
            let address = match config.local_address.trim() {
                "" => DEFAULT_LOCAL_HUB_ADDRESS,
                value => value,
            };
            let local = network.bind(address).map_err(|e| {
                anyhow::anyhow!("hub: could not bind local address {address} ({e})")
            })?;
            (config.log)(&format!("hub: also hosting on the local bus as {address}"));
            match remote {
                Some(remote) => Arc::new(RoutingBridge::new(
                    local,
                    Arc::new(remote) as Arc<dyn crate::bridge::Bridge>,
                )),
                None => Arc::new(RoutingBridge::local_only(local)),
            }
        }
        (None, Some(remote)) => Arc::new(remote),
        (None, None) => anyhow::bail!(
            "hub: no transport — configure a host link or attach a device-local network"
        ),
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
                label: (w.name != "medulla-worker").then(|| w.name.clone()),
                selected: false,
                // A spec describes a host this process just started; roles are
                // an operator choice made later, on the Hosts page.
                roles: Vec::new(),
                workspace: w.workspace.clone(),
                ..Default::default()
            })
            .collect(),
    ));
    let subscription_strategy = Arc::new(Mutex::new(config.subscription_strategy));

    (config.log)(&format!(
        "hub: connecting to {} ({} worker(s))",
        config.backend_url,
        config.workers.len()
    ));
    let catalog = Arc::new(config.agent_templates.clone());
    let socket = connect_harness(
        &config.backend_url,
        &config.jwt,
        super::socket::HarnessWiring {
            roster: roster.clone(),
            catalog: catalog.clone(),
            runner: runner.clone(),
            subscription_strategy: subscription_strategy.clone(),
            log: config.log.clone(),
            activity: Some(activity.clone()),
            // Installed here and nowhere else: this is the one place a hub's
            // uplink is built, so a bridge the host supplied on its config is
            // the bridge the socket serves.
            workflows: config.workflows.clone(),
        },
    )
    .await?;
    (config.log)("hub: connected + registered — relaying tasks to linked hosts");

    let handle = HubHandle::new(super::handle::HandleWiring {
        roster,
        socket: socket.clone(),
        address: hub_address,
        relay,
        catalog,
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
    })
}

/// Bring up the link and wrap it as a bridge.
///
/// The peer table is supplied by the caller rather than discovered: node names
/// and ids are issued together at enrollment (protocol §2), so by the time a hub
/// starts there is nothing left to resolve.
async fn connect_link(
    config: &HubLinkConfig,
    log: &super::types::HubLog,
) -> anyhow::Result<LinkBridge> {
    let handle = match &config.handle {
        Some(handle) => handle.clone(),
        None => {
            let mut link_config = LinkConfig::new(config.state_dir.clone());
            link_config.forwarder_endpoint = config.forwarder_endpoint.clone();
            link_config.peers = config
                .peers
                .iter()
                .map(|peer| PeerConfig {
                    node_id: peer.node_id,
                    pair_key: peer.pair_key.clone(),
                })
                .collect();
            Arc::new(
                Link::connect(link_config)
                    .await
                    .map_err(|e| anyhow::anyhow!("hub: could not bring up the host link ({e})"))?,
            )
        }
    };
    log(&format!(
        "hub: link up as {} ({} peer(s))",
        config.node_name,
        config.peers.len()
    ));
    Ok(LinkBridge::new(
        handle,
        LinkBridgeConfig {
            node_name: config.node_name.clone(),
            peers: config
                .peers
                .iter()
                .map(|peer| crate::bridge::LinkPeer {
                    name: peer.name.clone(),
                    node_id: peer.node_id,
                })
                .collect(),
        },
    ))
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
pub use types::HubLinkConfig;
pub use types::HubLinkPeer;
pub use types::HubSession;
pub use types::WorkerSpec;
