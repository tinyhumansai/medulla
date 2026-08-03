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
    /// The transport tasks are dispatched over.
    pub(super) relay: Arc<dyn Relay>,
    /// The agent-role catalog, read when re-advertising the roster.
    pub(super) catalog: Arc<Vec<crate::runtime::AgentTemplate>>,
    /// Sender/receiver correlation used for lightweight worker probes.
    pub(super) runner: Arc<super::super::runner::TaskRunner>,
    /// Latest capacity details keyed by stable worker id.
    pub(super) system_info: Arc<Mutex<HashMap<String, crate::protocol::WorkerSystemInfo>>>,
    /// Where roster mutations are narrated. An add that quietly does nothing is
    /// the hardest kind of failure to chase.
    pub(super) log: super::super::types::HubLog,
    /// Where the roster is written so it outlives the process. `None` keeps the
    /// old behaviour: in memory only, gone at exit.
    pub(super) persist: Option<super::super::types::RosterSink>,
    /// What the workers are doing, for the Agents view.
    pub(super) activity: super::super::ActivityLog,
    /// Live provider-subscription selection policy used by task dispatch.
    pub(super) subscription_strategy: super::super::roster::SharedSubscriptionStrategy,
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
    /// The hub's own link node name — surfaced to the operator because every
    /// worker must trust it before it will accept a task.
    pub address: String,
    /// The transport tasks are dispatched over.
    pub relay: Arc<dyn Relay>,
    /// The agent-role catalog, for resolving a worker's roles into what it
    /// advertises. Shared and read-only: registration reads it, nothing here
    /// writes it.
    pub catalog: Arc<Vec<crate::runtime::AgentTemplate>>,
    /// Runner used to request lightweight details from workers.
    pub runner: Arc<super::super::runner::TaskRunner>,
    /// Where roster mutations are narrated.
    pub log: super::super::types::HubLog,
    /// Where the roster is saved, when it is saved at all.
    pub persist: Option<super::super::types::RosterSink>,
    /// What the workers are doing, for the Agents view.
    pub activity: super::super::ActivityLog,
    /// Initial live provider-subscription selection policy.
    pub subscription_strategy: super::super::roster::SharedSubscriptionStrategy,
}
