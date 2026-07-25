//! Data types for the `hub_relay` module.
#[allow(unused_imports)]
use super::*;
/// The shared slot a [`BackendRuntime`](medulla::runtime::backend::BackendRuntime)
/// reads for its live worker roster; filled once the hub connects.
pub(crate) type HubSlot = Arc<Mutex<Option<HubHandle>>>;
