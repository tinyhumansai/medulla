//! Background tiny.place presence service for the TUI process.
//!
//! When the TUI config carries a `[tinyplace]` section, this service loads (or
//! mints) the machine identity, keeps it marked online, auto-accepts contact
//! requests from configured peers, and polls peer presence — surfacing all of it
//! into a shared [`TinyplaceObservation`] the [`App`](crate::ui::app::App) merges
//! into its render snapshot.
//!
//! This slice is deliberately **read-only / observational**: it does not
//! decrypt mailbox traffic or dispatch tasks to peers from the TUI. The task
//! dispatch path (and the interactive PTY wrapper) land separately; the headless
//! side of that already lives in [`crate::daemon`].

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::task::JoinHandle;

use crate::contacts::{AdmissionPolicy, ClientContacts, ContactDesk};
use crate::tinyplace::{
    load_or_create_identity, resolve_endpoint, spawn_presence_heartbeat, TinyplaceFileConfig,
};
use ::tinyplace::{Signer, TinyPlaceClient, TinyPlaceClientOptions};

use crate::config::TinyplaceConfig;
use crate::runtime::{AgentDescriptor, AgentPresence, TinyplaceIdentity};

const PRESENCE_POLL: Duration = Duration::from_secs(10);
const CONTACT_POLL: Duration = Duration::from_millis(1500);

/// What the service observes and the TUI renders.
#[derive(Debug, Clone, Default)]
pub struct TinyplaceObservation {
    /// This TUI's own tiny.place identity.
    pub identity: Option<TinyplaceIdentity>,
    /// The configured peer roster, tagged `harness=tinyplace`.
    pub roster: Vec<AgentDescriptor>,
    /// Latest presence per peer agent id.
    pub presence: HashMap<String, AgentPresence>,
    /// A problem the operator needs to know about, such as a failed pre-key
    /// publish — which leaves the identity reachable in the directory but unable
    /// to receive any DM.
    ///
    /// Carried here rather than printed: the consumers of this service own a
    /// terminal screen, and anything written to stdout or stderr under one lands
    /// on top of the UI and never clears.
    pub notice: Option<String>,
}

impl TinyplaceObservation {
    /// Merge this observation into a runtime snapshot in place.
    ///
    /// Overlays the tiny.place identity (when known), appends roster descriptors
    /// not already present by `id` (deduping so a peer configured statically and
    /// discovered live appears once), and upserts presence readings. Leaves the
    /// snapshot untouched for any field this observation has not populated.
    pub fn merge_into(&self, snapshot: &mut crate::runtime::RuntimeSnapshot) {
        if self.identity.is_some() {
            snapshot.tinyplace = self.identity.clone();
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

/// A running tiny.place background service. Dropping it aborts its loops.
pub struct TinyplaceService {
    observation: Arc<Mutex<TinyplaceObservation>>,
    contacts: ContactDesk,
    transport: crate::daemon::transport::SignalTransport,
    endpoint: String,
    handles: Vec<JoinHandle<()>>,
}

impl Drop for TinyplaceService {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.abort();
        }
    }
}

impl TinyplaceService {
    /// The shared observation the TUI reads.
    pub fn observation(&self) -> Arc<Mutex<TinyplaceObservation>> {
        self.observation.clone()
    }

    /// The encrypted Signal transport bound to this machine's wallet.
    ///
    /// Shared rather than rebuilt: a second transport on the same wallet would
    /// be a second writer to one Signal session store, and the double ratchet
    /// does not survive that.
    pub fn transport(&self) -> crate::daemon::transport::SignalTransport {
        self.transport.clone()
    }

    /// The tiny.place relay this service actually resolved to.
    ///
    /// Worth stating out loud at startup: two peers on different relays both
    /// start cleanly and report healthy, and the only symptom is that neither
    /// ever hears from the other. Printing it makes that one glance instead of
    /// an afternoon.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The incoming contact-request desk this service keeps current.
    ///
    /// The Sessions tab renders its queue and dispatches the operator's
    /// accept/decline/block decisions through it.
    pub fn contacts(&self) -> ContactDesk {
        self.contacts.clone()
    }

    /// Start the service from a `[tinyplace]` config section. Loads the identity,
    /// builds the client, seeds the roster, and spawns the presence/contact
    /// loops. Returns an error only if the identity cannot be established.
    pub fn start(config: &TinyplaceConfig) -> anyhow::Result<Self> {
        let env: HashMap<String, String> = std::env::vars().collect();
        let identity_dir = PathBuf::from(&config.identity_dir);
        let config_path = identity_dir.join("config.json");
        let (signer, tp_config) = load_or_create_identity(&config_path, &env)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        let endpoint = resolve_endpoint_with_config(&env, &tp_config, &config.base_url);
        let signer = Arc::new(signer);
        let client = TinyPlaceClient::new(TinyPlaceClientOptions {
            base_url: endpoint.clone(),
            signer: Some(signer.clone() as Arc<dyn Signer>),
            ..Default::default()
        });

        let identity_dir_path = identity_dir.clone();
        let transport = crate::daemon::transport::SignalTransport::new(
            client.clone(),
            &signer,
            &identity_dir_path,
        );

        let identity = TinyplaceIdentity {
            agent_id: signer.agent_id(),
            public_key: signer.public_key_base64(),
            handle: config.handle.clone(),
        };
        let roster = roster_from_peers(config);
        let peer_ids: Vec<String> = roster.iter().map(|a| a.id.clone()).collect();

        let observation = Arc::new(Mutex::new(TinyplaceObservation {
            identity: Some(identity),
            roster,
            presence: HashMap::new(),
            notice: None,
        }));

        let mut handles = Vec::new();

        // Publish Signal pre-keys. Without a published bundle a peer cannot run
        // X3DH against this identity, so every DM to it fails to establish a
        // session — the agent is reachable in the directory but unable to
        // receive anything, which looks from both ends like the message simply
        // vanished. The headless daemon has always done this as part of
        // onboarding; anything else holding an identity needs it too.
        handles.push({
            let transport = transport.clone();
            let signer = signer.clone();
            let observation = observation.clone();
            tokio::spawn(async move {
                if let Err(err) = transport.publish_keys(&signer).await {
                    if let Ok(mut obs) = observation.lock() {
                        obs.notice = Some(format!(
                            "pre-key publish failed ({err}) — peers cannot open an encrypted channel to this agent"
                        ));
                    }
                }
            })
        });

        handles.push(spawn_presence_heartbeat(client.clone(), CONTACT_POLL));

        // Contact admission. `accept_contacts` maps onto the admission policy
        // directly — the shipped default `"peers"` is the same fail-closed
        // allowlist as before, and `"all"` the same open one. What is new is
        // that requests policy does *not* admit are now queued for the operator
        // in the Sessions tab instead of being silently ignored, and that an
        // unrecognised value closes to `manual` rather than falling through.
        let contacts = ContactDesk::new(
            Arc::new(ClientContacts::new(client.clone())),
            AdmissionPolicy::parse(&config.accept_contacts),
            peer_ids.iter().cloned().collect::<HashSet<_>>(),
        );
        handles.push(contacts.spawn_poll(CONTACT_POLL));

        // Presence poll: refresh peer online status into the observation.
        if !peer_ids.is_empty() {
            let observation = observation.clone();
            let client = client.clone();
            handles.push(tokio::spawn(async move {
                loop {
                    if let Ok(response) = client.presence.query(&peer_ids).await {
                        let at = now_ms();
                        let mut obs = observation.lock().unwrap();
                        for status in response.presence {
                            obs.presence.insert(
                                status.crypto_id.clone(),
                                AgentPresence {
                                    online: status.online,
                                    detail: None,
                                    at,
                                },
                            );
                        }
                    }
                    tokio::time::sleep(PRESENCE_POLL).await;
                }
            }));
        }

        Ok(TinyplaceService {
            observation,
            contacts,
            transport,
            endpoint,
            handles,
        })
    }
}

fn roster_from_peers(config: &TinyplaceConfig) -> Vec<AgentDescriptor> {
    config
        .peers
        .iter()
        .map(|peer| {
            let mut metadata = serde_json::Map::new();
            metadata.insert(
                "harness".to_string(),
                Value::String("tinyplace".to_string()),
            );
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
                tags: peer.tags.clone().unwrap_or_default(),
                metadata,
            }
        })
        .collect()
}

/// The TUI's `[tinyplace].baseUrl` wins unless an env override or the tinyplace
/// config file's endpoint is set (mirroring the CLI's precedence, with the TUI
/// section as the final default rather than the hard-coded endpoint).
fn resolve_endpoint_with_config(
    env: &HashMap<String, String>,
    tp_config: &TinyplaceFileConfig,
    tui_base_url: &str,
) -> String {
    // Env + config-file endpoint take precedence via the shared resolver; when
    // neither is set the resolver returns the DEFAULT_ENDPOINT, in which case we
    // prefer the TUI's explicit base_url.
    let resolved = resolve_endpoint(env, tp_config);
    if resolved == crate::config::default_tinyplace_base_url(env) && !tui_base_url.is_empty() {
        tui_base_url.to_string()
    } else {
        resolved
    }
}

/// Milliseconds since the Unix epoch. Delegates to the shared [`crate::clock`]
/// helper.
fn now_ms() -> i64 {
    crate::clock::now_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Peer;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn merge_into_overlays_identity_dedups_roster_and_upserts_presence() {
        use crate::runtime::{AgentDescriptor, AgentPresence, RuntimeSnapshot};

        let mut snapshot = RuntimeSnapshot {
            roster: vec![AgentDescriptor {
                id: "peer-1".into(),
                ..Default::default()
            }],
            ..Default::default()
        };

        // A duplicate id (peer-1) must not be appended twice; peer-2 is new.
        let mut obs = TinyplaceObservation {
            roster: vec![
                AgentDescriptor {
                    id: "peer-1".into(),
                    ..Default::default()
                },
                AgentDescriptor {
                    id: "peer-2".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        obs.presence
            .insert("peer-1".into(), AgentPresence::default());

        obs.merge_into(&mut snapshot);

        let ids: Vec<&str> = snapshot.roster.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["peer-1", "peer-2"]);
        assert!(snapshot.presence.contains_key("peer-1"));
    }

    #[test]
    fn endpoint_prefers_env_override_over_tui_base_url() {
        let tp = TinyplaceFileConfig::default();
        // An env override resolves to something other than the DEFAULT_ENDPOINT,
        // so it wins over the TUI base_url.
        let e = env(&[("TINYPLACE_ENDPOINT", "https://override")]);
        assert_eq!(
            resolve_endpoint_with_config(&e, &tp, "https://tui-default"),
            "https://override"
        );
    }

    #[test]
    fn endpoint_falls_back_to_tui_base_url_then_default() {
        let tp = TinyplaceFileConfig::default();
        // No env/config endpoint → resolver returns DEFAULT_ENDPOINT, so the
        // explicit TUI base_url is used.
        assert_eq!(
            resolve_endpoint_with_config(&HashMap::new(), &tp, "https://tui"),
            "https://tui"
        );
        // Empty TUI base_url → the DEFAULT_ENDPOINT stands.
        assert_eq!(
            resolve_endpoint_with_config(&HashMap::new(), &tp, ""),
            crate::tinyplace::DEFAULT_ENDPOINT
        );
    }

    #[test]
    fn roster_from_bare_peer_uses_id_fallbacks() {
        let config = TinyplaceConfig {
            peers: vec![Peer {
                id: "peer-x".to_string(),
                name: None,
                handle: None,
                address: None,
                tags: None,
                description: None,
                protocol: "task".to_string(),
            }],
            ..Default::default()
        };
        let roster = roster_from_peers(&config);
        assert_eq!(roster.len(), 1);
        let d = &roster[0];
        // name falls back to the id when neither name nor handle is set.
        assert_eq!(d.name, "peer-x");
        assert_eq!(d.description, "");
        assert!(d.tags.is_empty());
        // harness tag is always present; no handle/address keys for a bare peer.
        assert_eq!(
            d.metadata.get("harness").and_then(|v| v.as_str()),
            Some("tinyplace")
        );
        assert!(d.metadata.get("handle").is_none());
        assert!(d.metadata.get("address").is_none());
    }

    #[test]
    fn roster_from_peer_prefers_handle_when_name_absent() {
        let config = TinyplaceConfig {
            peers: vec![Peer {
                id: "peer-y".to_string(),
                name: None,
                handle: Some("@handle".to_string()),
                address: Some("addr".to_string()),
                tags: None,
                description: None,
                protocol: "task".to_string(),
            }],
            ..Default::default()
        };
        let roster = roster_from_peers(&config);
        assert_eq!(roster[0].name, "@handle");
        assert_eq!(
            roster[0].metadata.get("handle").and_then(|v| v.as_str()),
            Some("@handle")
        );
        assert_eq!(
            roster[0].metadata.get("address").and_then(|v| v.as_str()),
            Some("addr")
        );
    }

    #[test]
    fn now_ms_is_positive() {
        assert!(now_ms() > 0);
    }
}
