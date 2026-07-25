//! In-memory bridge for agents running within the same process.
//!
//! The network is an explicit value rather than a global registry. Embedders
//! decide which local agents can see each other by handing them the same
//! [`LocalBridgeNetwork`], and tests remain isolated from one another.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;

use crate::daemon::transport::InboundMessage;

use super::Bridge;

type Inbox = Arc<Mutex<VecDeque<InboundMessage>>>;
type WeakInbox = Weak<Mutex<VecDeque<InboundMessage>>>;

/// An isolated collection of device-local bridge endpoints.
#[derive(Clone, Default)]
pub struct LocalBridgeNetwork {
    endpoints: Arc<Mutex<HashMap<String, WeakInbox>>>,
}

impl LocalBridgeNetwork {
    /// Create an empty local network.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind one address to this network.
    ///
    /// # Errors
    ///
    /// Returns an error for a blank address or an address already bound by
    /// another endpoint. This prevents two consumers from destructively draining
    /// the same logical inbox.
    pub fn bind(&self, address: impl Into<String>) -> Result<LocalBridge, String> {
        let address = address.into();
        if address.trim().is_empty() {
            return Err("local bridge address cannot be blank".to_string());
        }
        let mut endpoints = self
            .endpoints
            .lock()
            .map_err(|_| "local bridge registry is unavailable".to_string())?;
        if endpoints.get(&address).and_then(Weak::upgrade).is_some() {
            return Err(format!("local bridge address is already bound: {address}"));
        }
        let inbox = Arc::new(Mutex::new(VecDeque::new()));
        endpoints.insert(address.clone(), Arc::downgrade(&inbox));
        Ok(LocalBridge {
            address,
            inbox,
            network: self.clone(),
        })
    }

    /// Whether an address is currently bound.
    fn contains(&self, address: &str) -> bool {
        self.endpoints
            .lock()
            .ok()
            .and_then(|endpoints| endpoints.get(address).and_then(Weak::upgrade))
            .is_some()
    }

    /// Look up a target inbox without exposing the registry lock across awaits.
    fn inbox(&self, address: &str) -> Option<Inbox> {
        self.endpoints
            .lock()
            .ok()
            .and_then(|endpoints| endpoints.get(address).and_then(Weak::upgrade))
    }
}

/// One endpoint on a [`LocalBridgeNetwork`].
///
/// Clones share the same inbox and address. The endpoint unregisters when its
/// last clone is dropped, allowing the address to be safely rebound.
#[derive(Clone)]
pub struct LocalBridge {
    address: String,
    inbox: Inbox,
    network: LocalBridgeNetwork,
}

impl LocalBridge {
    /// This local endpoint's address.
    pub fn address(&self) -> &str {
        &self.address
    }
}

#[async_trait]
impl Bridge for LocalBridge {
    async fn send(&self, to: &str, body: &str) -> Result<(), String> {
        let inbox = self
            .network
            .inbox(to)
            .ok_or_else(|| format!("local bridge endpoint is not available: {to}"))?;
        inbox
            .lock()
            .map_err(|_| format!("local bridge inbox is unavailable: {to}"))?
            .push_back(InboundMessage {
                from: self.address.clone(),
                text: body.to_string(),
            });
        Ok(())
    }

    async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage> {
        if limit <= 0 {
            return Vec::new();
        }
        let Ok(mut inbox) = self.inbox.lock() else {
            return Vec::new();
        };
        let count = usize::try_from(limit)
            .unwrap_or(usize::MAX)
            .min(inbox.len());
        inbox.drain(..count).collect()
    }

    async fn request_contact(&self, peer: &str) -> Result<(), String> {
        self.network
            .contains(peer)
            .then_some(())
            .ok_or_else(|| format!("local bridge endpoint is not available: {peer}"))
    }

    async fn resolve_handle(&self, name: &str) -> Option<String> {
        let name = name.trim();
        if self.network.contains(name) {
            return Some(name.to_string());
        }
        let bare = name.trim_start_matches('@');
        [bare.to_string(), format!("@{bare}")]
            .into_iter()
            .find(|candidate| self.network.contains(candidate))
    }

    async fn contact_accepted(&self, peer: &str) -> bool {
        self.network.contains(peer)
    }

    async fn reset_session(&self, _peer: &str) {}
}
