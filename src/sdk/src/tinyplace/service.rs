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
#[path = "service_tests.rs"]
mod tests;

mod types;
pub use types::TinyplaceObservation;
pub use types::TinyplaceService;
