//! Data types for the `service` module.
#[allow(unused_imports)]
use super::*;
/// One observed incoming request, as the relay reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct IncomingRequest {
    /// The requesting peer's cryptoId.
    pub agent_id: String,
    /// The peer's directory handle, when known.
    pub handle: Option<String>,
}
/// A [`ContactRelay`] backed by the tiny.place SDK client.
pub struct ClientContacts {
    pub(super) client: ::tinyplace::TinyPlaceClient,
}
/// A clock in epoch ms (injectable for tests).
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;
