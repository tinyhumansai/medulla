//! Remote bridge backed by tiny.place's encrypted Signal transport.

use async_trait::async_trait;

use crate::daemon::transport::{InboundMessage, SignalTransport};

use super::Bridge;

/// A remote bridge that sends encrypted messages through tiny.place.
#[derive(Clone)]
pub struct TinyplaceBridge {
    transport: SignalTransport,
}

impl TinyplaceBridge {
    /// Wrap an initialized tiny.place Signal transport.
    pub fn new(transport: SignalTransport) -> Self {
        Self { transport }
    }

    /// This tiny.place endpoint's crypto address.
    pub fn address(&self) -> &str {
        self.transport.agent_id()
    }

    /// Access the transport for tiny.place-specific setup such as key
    /// maintenance and directory registration.
    pub fn transport(&self) -> &SignalTransport {
        &self.transport
    }

    /// Consume the bridge and return its underlying transport.
    pub fn into_transport(self) -> SignalTransport {
        self.transport
    }
}

#[async_trait]
impl Bridge for TinyplaceBridge {
    async fn send(&self, to: &str, body: &str) -> Result<(), String> {
        self.transport.send(to, body).await
    }

    async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage> {
        self.transport.drain_inbox(limit).await
    }

    async fn wait_for_inbox(&self, poll: std::time::Duration) {
        self.transport.wait_for_inbox(poll).await
    }

    async fn request_contact(&self, peer: &str) -> Result<(), String> {
        self.transport.request_contact(peer).await
    }

    async fn resolve_handle(&self, name: &str) -> Option<String> {
        self.transport.resolve_handle(name).await
    }

    async fn contact_accepted(&self, peer: &str) -> bool {
        self.transport.contact_accepted(peer).await
    }

    async fn reset_session(&self, peer: &str) {
        self.transport.reset_session(peer).await;
    }
}
