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
}
