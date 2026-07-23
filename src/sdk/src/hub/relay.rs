//! The `Relay` seam: the tiny.place operations the sender-runner needs, behind a
//! trait so the runner is unit-testable with a fake while `SignalTransport`
//! provides the real, Signal-encrypted implementation.

use async_trait::async_trait;

use crate::daemon::transport::{InboundMessage, SignalTransport};

/// The minimal encrypted-transport surface the
/// [`TaskRunner`](super::TaskRunner) drives: send a frame, drain decrypted
/// inbound frames, and (idempotently) request a contact so a peer accepts the
/// first DM.
#[async_trait]
pub trait Relay: Send + Sync {
    /// Encrypt and send `body` to `to` (a base58 cryptoId or `@handle`).
    async fn send(&self, to: &str, body: &str) -> Result<(), String>;

    /// Destructively read, decrypt, and acknowledge up to `limit` inbound DMs,
    /// returning their plaintext bodies. Acknowledged messages are not
    /// redelivered, so a single caller must fan them out to all waiters.
    async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage>;

    /// Ask `peer` for a contact relationship. Safe to call repeatedly — the
    /// directory refuses a DM to a non-contact, and requesting an existing
    /// contact is harmless.
    async fn request_contact(&self, peer: &str) -> Result<(), String>;

    /// Resolve an `@handle` to its cryptoId, or `None` when unknown.
    ///
    /// Defaulted so a fake relay only implements it when the test cares.
    async fn resolve_handle(&self, _name: &str) -> Option<String> {
        None
    }

    /// Whether `peer` has *accepted* the contact request. A request only creates
    /// a pending edge, so the runner waits on this before its first send.
    async fn contact_accepted(&self, peer: &str) -> bool;

    /// Drop the local Signal session with `peer` so the next send re-runs X3DH.
    /// The runner calls this to recover a desynced (e.g. post-restart) peer.
    async fn reset_session(&self, peer: &str);
}

#[async_trait]
impl Relay for SignalTransport {
    async fn send(&self, to: &str, body: &str) -> Result<(), String> {
        SignalTransport::send(self, to, body).await
    }

    async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage> {
        SignalTransport::drain_inbox(self, limit).await
    }

    async fn request_contact(&self, peer: &str) -> Result<(), String> {
        SignalTransport::request_contact(self, peer).await
    }

    async fn resolve_handle(&self, name: &str) -> Option<String> {
        SignalTransport::resolve_handle(self, name).await
    }

    async fn contact_accepted(&self, peer: &str) -> bool {
        SignalTransport::contact_accepted(self, peer).await
    }

    async fn reset_session(&self, peer: &str) {
        SignalTransport::reset_session(self, peer).await
    }
}
