//! Data types for the `transport` module.
#[allow(unused_imports)]
use super::*;
/// One decrypted inbound DM.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub from: String,
    pub text: String,
}
/// Encrypted transport bound to one machine wallet.
#[derive(Clone)]
pub struct SignalTransport {
    pub(super) client: TinyPlaceClient,
    pub(super) session: Arc<SignalSession>,
    pub(super) store: Arc<FileSessionStore>,
    pub(super) our_agent_id: String,
    pub(super) our_ed25519_pub: [u8; 32],
    /// Serializes ratchet-touching ops (encrypt/decrypt) on this wallet.
    pub(super) lock: Arc<Mutex<()>>,
    /// Envelopes the relay pushed over the stream socket, waiting to be
    /// decrypted by the next drain.
    pub(super) push: crate::daemon::listener::PushInbox,
    /// Message ids already handled, so a snapshot replay or a reconciliation
    /// fetch racing a delete cannot process one twice.
    pub(super) seen: crate::daemon::listener::SeenIds,
    /// When the mailbox was last fetched over HTTPS, so reconciliation can run
    /// on a schedule even while the socket looks healthy.
    pub(super) last_fetch: Arc<std::sync::Mutex<std::time::Instant>>,
}
