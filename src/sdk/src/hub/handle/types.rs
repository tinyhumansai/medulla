//! Data types for the `handle` module.
#[allow(unused_imports)]
use super::*;
/// A live control handle over the hub's roster, held by the TUI. Mutations
/// re-register the roster with the backend so a newly-added worker becomes a
/// delegation target (and a removed one stops being one) without a restart.
#[derive(Clone)]
pub struct HubHandle {
    pub(super) roster: SharedRoster,
    pub(super) socket: Client,
    pub(super) address: String,
    pub(super) public_key: String,
    /// The encrypted transport, used to open a contact edge with a peer the
    /// moment it is added rather than at first dispatch.
    pub(super) relay: Arc<dyn Relay>,
    /// Sender/receiver correlation used for lightweight worker probes.
    pub(super) runner: Arc<super::super::runner::TaskRunner>,
    /// Latest capacity details keyed by stable worker id.
    pub(super) system_info: Arc<Mutex<HashMap<String, crate::tinyplace::WorkerSystemInfo>>>,
    /// Where roster mutations are narrated. An add that quietly does nothing is
    /// the hardest kind of failure to chase.
    pub(super) log: super::super::types::HubLog,
    /// Where the roster is written so it outlives the process. `None` keeps the
    /// old behaviour: in memory only, gone at exit.
    pub(super) persist: Option<super::super::types::RosterSink>,
    /// What the workers are doing, for the Agents view.
    pub(super) activity: super::super::ActivityLog,
}
/// Everything a [`HubHandle`] is built from.
///
/// A struct rather than a parameter list: the handle needs the roster, the
/// uplink, the hub's own identity and three side-channels, and eight positional
/// arguments is a place where two of the same type get silently transposed.
pub(in super::super) struct HandleWiring {
    /// The shared roster this handle mutates.
    pub roster: SharedRoster,
    /// The uplink to re-register through.
    pub socket: Client,
    /// The hub's own tiny.place address — surfaced to the operator because every
    /// worker must trust it before it will accept a task.
    pub address: String,
    /// The hub's own identity public key.
    pub public_key: String,
    /// The encrypted transport, for opening contact edges.
    pub relay: Arc<dyn Relay>,
    /// Runner used to request lightweight details from workers.
    pub runner: Arc<super::super::runner::TaskRunner>,
    /// Where roster mutations are narrated.
    pub log: super::super::types::HubLog,
    /// Where the roster is saved, when it is saved at all.
    pub persist: Option<super::super::types::RosterSink>,
    /// What the workers are doing, for the Agents view.
    pub activity: super::super::ActivityLog,
}
