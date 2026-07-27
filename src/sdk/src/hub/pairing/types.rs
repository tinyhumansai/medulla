//! Data types for the `pairing` module.
#[allow(unused_imports)]
use super::*;

/// A running inbound-pairing poll, stopped on drop.
///
/// Held by the [`HubSession`](crate::hub::HubSession) so a session that ends does
/// not leave a loop talking to the relay on a dead identity's behalf.
pub(crate) struct PairingPoll(pub(super) tokio::task::JoinHandle<()>);

impl Drop for PairingPoll {
    fn drop(&mut self) {
        self.0.abort();
    }
}
