//! Data types for the `roster` module.
#[allow(unused_imports)]
use super::*;
/// One worker in the live roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubWorker {
    /// The `agentId` the backend targets (defaults to the address).
    pub id: String,
    /// tiny.place address (base58 cryptoId or `@handle`).
    pub address: String,
    /// Coding-agent harness the worker runs.
    pub harness: String,
    /// Optional human label.
    pub label: Option<String>,
    /// Whether this worker is the currently-selected default.
    pub selected: bool,
}
/// The roster shared between the socket layer and the [`HubHandle`].
pub type SharedRoster = Arc<Mutex<Vec<HubWorker>>>;
