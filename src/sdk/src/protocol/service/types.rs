//! Data types for the `service` module.
#[allow(unused_imports)]
use super::*;

/// What the service observes and the TUI renders.
#[derive(Debug, Clone, Default)]
pub struct LinkObservation {
    /// This endpoint's own host-link identity.
    pub identity: Option<LinkIdentity>,
    /// The configured peer roster, tagged `harness=link`.
    pub roster: Vec<AgentDescriptor>,
    /// Latest presence per peer id, derived from link liveness (§6.2).
    pub presence: HashMap<String, AgentPresence>,
    /// A problem the operator needs to know about.
    ///
    /// Carried here rather than printed: the consumers of this service own a
    /// terminal screen, and anything written to stdout or stderr under one lands
    /// on top of the UI and never clears.
    pub notice: Option<String>,
}

/// A running host-link background service. Dropping it aborts its loops and
/// releases the link identity lock.
pub struct LinkService {
    pub(super) observation: Arc<Mutex<LinkObservation>>,
    pub(super) link: Arc<LinkHandle>,
    pub(super) endpoint: String,
    pub(super) handles: Vec<JoinHandle<()>>,
}
