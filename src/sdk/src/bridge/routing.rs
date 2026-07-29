//! Address-routing bridge: one endpoint that speaks to device-local peers over
//! the in-memory bus and to everyone else over tiny.place.
//!
//! This is what lets a single process be both orchestrator and host. The hub
//! dispatches through one [`Bridge`], the embedded daemon binds a local address
//! on the same [`LocalBridgeNetwork`](super::LocalBridgeNetwork), and work for
//! this machine never leaves it — no relay round-trip, no contact graph, no
//! encryption handshake — while a remote worker's address falls through to the
//! tiny.place transport unchanged.
//!
//! Routing is by *binding*, not by spelling: an address routes locally exactly
//! when the local network currently has an endpoint for it. A worker that goes
//! away therefore stops matching, rather than silently black-holing its traffic
//! on a dead in-memory inbox.

use async_trait::async_trait;

use crate::daemon::transport::InboundMessage;

use super::{Bridge, LocalBridge};

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;

/// A bridge that picks the local bus or a remote transport per address.
#[derive(Clone)]
pub struct RoutingBridge {
    /// This process's endpoint on the device-local bus.
    local: LocalBridge,
    /// The remote transport for addresses that are not bound locally. `None`
    /// makes this a local-only bridge: remote addresses fail loudly instead of
    /// appearing to send.
    remote: Option<std::sync::Arc<dyn Bridge>>,
}

impl RoutingBridge {
    /// Route between `local` and `remote`.
    pub fn new(local: LocalBridge, remote: std::sync::Arc<dyn Bridge>) -> Self {
        Self {
            local,
            remote: Some(remote),
        }
    }

    /// Route local addresses only.
    ///
    /// Used when this device has no tiny.place identity: the orchestrator still
    /// reaches its own host, and a remote address reports that plainly rather
    /// than being accepted and dropped.
    pub fn local_only(local: LocalBridge) -> Self {
        Self {
            local,
            remote: None,
        }
    }

    /// This endpoint's device-local address.
    pub fn local_address(&self) -> &str {
        self.local.address()
    }

    /// The remote endpoint's address, when a remote transport is attached.
    pub fn remote(&self) -> Option<&std::sync::Arc<dyn Bridge>> {
        self.remote.as_ref()
    }

    /// Whether `address` currently names an endpoint on the local bus.
    ///
    /// Resolution goes through the local bridge's handle lookup, so a
    /// `@handle`-style local name matches the endpoint it was bound under.
    pub async fn is_local(&self, address: &str) -> bool {
        self.local.resolve_handle(address).await.is_some()
    }

    /// The remote transport, or a routing error naming the unreachable address.
    fn require_remote(&self, address: &str) -> Result<&std::sync::Arc<dyn Bridge>, String> {
        self.remote.as_ref().ok_or_else(|| {
            format!("no remote transport is configured for a non-local address: {address}")
        })
    }
}

#[async_trait]
impl Bridge for RoutingBridge {
    async fn send(&self, to: &str, body: &str) -> Result<(), String> {
        if self.is_local(to).await {
            return self.local.send(to, body).await;
        }
        self.require_remote(to)?.send(to, body).await
    }

    async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage> {
        if limit <= 0 {
            return Vec::new();
        }
        // Local first, and the remote drain is bounded by what is left. Both
        // inboxes are destructive, so over-draining one would strand frames the
        // caller asked to be limited to.
        let mut messages = self.local.drain_inbox(limit).await;
        let remaining = limit - messages.len() as i64;
        if remaining > 0 {
            if let Some(remote) = &self.remote {
                messages.extend(remote.drain_inbox(remaining).await);
            }
        }
        messages
    }

    async fn wait_for_inbox(&self, poll: std::time::Duration) {
        // Only the remote side is worth blocking on: the local bus is
        // in-process, so anything arriving there is already here by the time
        // the next tick comes round.
        match &self.remote {
            Some(remote) => remote.wait_for_inbox(poll).await,
            None => tokio::time::sleep(poll).await,
        }
    }

    async fn request_contact(&self, peer: &str) -> Result<(), String> {
        if self.is_local(peer).await {
            return self.local.request_contact(peer).await;
        }
        self.require_remote(peer)?.request_contact(peer).await
    }

    async fn resolve_handle(&self, name: &str) -> Option<String> {
        if let Some(address) = self.local.resolve_handle(name).await {
            return Some(address);
        }
        self.remote.as_ref()?.resolve_handle(name).await
    }

    /// Split by where the address lives. A device-local one is answered here as
    /// live — it names something bound in this process, so it has no heartbeat
    /// to read and needs none. Only the rest are forwarded to the remote.
    async fn presence(&self, addresses: &[String]) -> std::collections::HashMap<String, bool> {
        // A device-local address is answered here, not asked of the relay. It
        // names something bound in this process, so its liveness is knowable
        // exactly — and tiny.place has never heard of it, because it is not a
        // tiny.place identity and never heartbeats. Forwarding it returns "no
        // record", which reads as offline and takes every host on this machine
        // out of the roster.
        let mut out = std::collections::HashMap::new();
        let mut remote_addresses = Vec::new();
        for address in addresses {
            if self.is_local(address).await {
                out.insert(address.clone(), true);
            } else {
                remote_addresses.push(address.clone());
            }
        }
        if let Some(remote) = &self.remote {
            if !remote_addresses.is_empty() {
                out.extend(remote.presence(&remote_addresses).await);
            }
        }
        out
    }

    async fn is_device_local(&self, address: &str) -> bool {
        self.is_local(address).await
    }

    async fn contact_accepted(&self, peer: &str) -> bool {
        if self.is_local(peer).await {
            return true;
        }
        match &self.remote {
            Some(remote) => remote.contact_accepted(peer).await,
            None => false,
        }
    }

    async fn reset_session(&self, peer: &str) {
        // A local endpoint has no session state, so resetting one is a no-op —
        // but a *remote* reset must not be skipped just because the peer is
        // momentarily unresolvable, which is exactly when the hub asks for one.
        if self.is_local(peer).await {
            return;
        }
        if let Some(remote) = &self.remote {
            remote.reset_session(peer).await;
        }
    }
}
