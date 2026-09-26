//! Background host-link service for the TUI process.
//!
//! When the TUI config carries a `[link]` section, this service brings the link
//! up and keeps a shared [`LinkObservation`] current — this endpoint's identity,
//! the configured peer roster, and each peer's presence — which the
//! [`App`](crate::ui::app::App) merges into its render snapshot.
//!
//! Presence here is *link liveness* (protocol §6.2), not a relay's online flag:
//! a peer counts as online when a datagram from it arrived recently. That makes
//! it server-free and honest — it reports what this endpoint has actually heard
//! rather than what a third party asserts — and it is derived, so an endpoint
//! cannot claim to be up.
//!
//! This slice is deliberately **read-only / observational**: it does not
//! dispatch tasks. That path is the hub's ([`crate::hub`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::task::JoinHandle;

use medulla_link::keys::NodeId;
use medulla_link::{Link, LinkConfig as LinkTransportConfig, LinkHandle, Liveness};

use crate::config::LinkConfig;
use crate::runtime::{AgentDescriptor, AgentPresence, LinkIdentity};

/// How often peer liveness is folded into the observation.
///
/// Well under the 9-second `Live` window (§6.2), so a peer going quiet shows up
/// as degraded within a couple of ticks rather than a whole window later.
const PRESENCE_POLL: Duration = Duration::from_secs(2);

impl LinkObservation {
    /// Merge this observation into a runtime snapshot in place.
    ///
    /// Overlays the link identity (when known), appends roster descriptors not
    /// already present by `id` (deduping so a peer configured statically and
    /// discovered live appears once), and upserts presence readings. Leaves the
    /// snapshot untouched for any field this observation has not populated.
    pub fn merge_into(&self, snapshot: &mut crate::runtime::RuntimeSnapshot) {
        if self.identity.is_some() {
            snapshot.link = self.identity.clone();
        }
        for descriptor in &self.roster {
            if !snapshot.roster.iter().any(|a| a.id == descriptor.id) {
                snapshot.roster.push(descriptor.clone());
            }
        }
        for (id, presence) in &self.presence {
            snapshot.presence.insert(id.clone(), presence.clone());
        }
    }
}

impl Drop for LinkService {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.abort();
        }
    }
}

impl LinkService {
    /// The shared observation the TUI reads.
    pub fn observation(&self) -> Arc<Mutex<LinkObservation>> {
        self.observation.clone()
    }

    /// The running link, for callers that need to send over it.
    ///
    /// Shared rather than reopened: the identity is held under an exclusive lock
    /// and a second `Link` on the same node would draw sequences from a second
    /// counter under one AEAD key, which reuses nonces (protocol §3.1).
    pub fn link(&self) -> &Arc<LinkHandle> {
        &self.link
    }

    /// The forwarder this service actually resolved to.
    ///
    /// Worth stating out loud at startup: two endpoints on different forwarders
    /// both start cleanly and report healthy, and the only symptom is that
    /// neither ever hears from the other.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Start the service from a `[link]` config section.
    ///
    /// Brings the link up, seeds the roster from the configured peers, and
    /// spawns the presence loop.
    ///
    /// # Errors
    ///
    /// When the link identity is missing, malformed, or already held by another
    /// process — all of which mean this endpoint cannot serve, and none of which
    /// a retry would fix.
    pub async fn start(config: &LinkConfig) -> anyhow::Result<Self> {
        // The peer table comes from `node.json`, not from configuration: a
        // session needs a pair key, and the pair key is never written to a
        // config file (protocol §7.1).
        let state_dir = PathBuf::from(&config.state_dir);
        let enrolled_node_name =
            medulla_link::keys::read_node_state(&medulla_link::keys::node_path(&state_dir))
                .ok()
                .map(|state| state.node_id.to_string());
        let link = Arc::new(
            Link::connect(LinkTransportConfig::new(state_dir))
                .await
                .map_err(|e| anyhow::anyhow!("host link: {e}"))?,
        );

        let identity = LinkIdentity {
            node_name: config
                .node_name
                .clone()
                .or(enrolled_node_name)
                .unwrap_or_default(),
            forwarder: config.forwarder_url.clone(),
        };
        // Presence is keyed by the roster's own peer id, so the descriptor the
        // TUI renders and the reading that lights it up agree without the view
        // having to know anything about node ids.
        let mut by_node: Vec<(NodeId, String)> = config
            .peers
            .iter()
            .filter_map(|peer| Some((parse_node_id(peer.node_id.as_deref()?)?, peer.id.clone())))
            .collect();
        for peer in link.peers() {
            if !by_node.iter().any(|(node_id, _)| node_id == peer) {
                by_node.push((*peer, peer.to_string()));
            }
        }

        let observation = Arc::new(Mutex::new(LinkObservation {
            identity: Some(identity),
            roster: roster_from_peers_and_nodes(config, &by_node),
            presence: HashMap::new(),
            notice: None,
        }));

        let mut handles = Vec::new();
        if !by_node.is_empty() {
            let observation = observation.clone();
            let link = link.clone();
            handles.push(tokio::spawn(async move {
                loop {
                    let status = link.status();
                    let at = crate::clock::now_millis();
                    if let Ok(mut obs) = observation.lock() {
                        for (node_id, id) in &by_node {
                            let liveness = status.peer(node_id).map(|peer| peer.liveness);
                            obs.presence.insert(
                                id.clone(),
                                AgentPresence {
                                    // `Degraded` still counts as online: the link
                                    // is retransmitting and the peer has not
                                    // failed (§6.2). Only sustained silence reads
                                    // as offline.
                                    online: matches!(
                                        liveness,
                                        Some(Liveness::Live) | Some(Liveness::Degraded)
                                    ),
                                    detail: liveness.map(describe_liveness),
                                    at,
                                },
                            );
                        }
                    }
                    tokio::time::sleep(PRESENCE_POLL).await;
                }
            }));
        }

        Ok(LinkService {
            observation,
            link,
            endpoint: config.forwarder_url.clone(),
            handles,
        })
    }
}

/// A one-word rendering of a peer's liveness, for the roster row.
fn describe_liveness(liveness: Liveness) -> String {
    match liveness {
        Liveness::Live => "live".to_string(),
        Liveness::Degraded => "degraded".to_string(),
        Liveness::Offline => "offline".to_string(),
    }
}

/// Parse a 16-byte node id from its hex form, or `None` when it is malformed.
fn parse_node_id(hex: &str) -> Option<NodeId> {
    let hex = hex.trim();
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(NodeId(bytes))
}

/// Project the configured peers into roster descriptors the Agents view renders.
fn roster_from_peers(config: &LinkConfig) -> Vec<AgentDescriptor> {
    config
        .peers
        .iter()
        .map(|peer| {
            let mut metadata = serde_json::Map::new();
            metadata.insert("harness".to_string(), Value::String("link".to_string()));
            if let Some(handle) = &peer.handle {
                metadata.insert("handle".to_string(), Value::String(handle.clone()));
            }
            if let Some(address) = &peer.address {
                metadata.insert("address".to_string(), Value::String(address.clone()));
            }
            AgentDescriptor {
                id: peer.id.clone(),
                name: peer
                    .name
                    .clone()
                    .or_else(|| peer.handle.clone())
                    .unwrap_or_else(|| peer.id.clone()),
                description: peer.description.clone().unwrap_or_default(),
                availability: String::new(),
                workspace_id: None,
                host_id: None,
                template_id: None,
                tags: peer.tags.clone().unwrap_or_default(),
                metadata,
            }
        })
        .collect()
}

/// Add enrolled link peers that have no configured display metadata.
fn roster_from_peers_and_nodes(
    config: &LinkConfig,
    by_node: &[(NodeId, String)],
) -> Vec<AgentDescriptor> {
    let mut roster = roster_from_peers(config);
    for (_, id) in by_node {
        if roster.iter().any(|descriptor| descriptor.id == *id) {
            continue;
        }
        let mut metadata = serde_json::Map::new();
        metadata.insert("harness".to_string(), Value::String("link".to_string()));
        roster.push(AgentDescriptor {
            id: id.clone(),
            name: id.clone(),
            description: String::new(),
            availability: String::new(),
            workspace_id: None,
            host_id: None,
            template_id: None,
            tags: Vec::new(),
            metadata,
        });
    }
    roster
}

#[cfg(test)]
mod tests;

mod types;
pub use types::LinkObservation;
pub use types::LinkService;
