//! Data types for the `service` module.
#[allow(unused_imports)]
use super::*;
/// What the service observes and the TUI renders.
#[derive(Debug, Clone, Default)]
pub struct TinyplaceObservation {
    /// This TUI's own tiny.place identity.
    pub identity: Option<TinyplaceIdentity>,
    /// The configured peer roster, tagged `harness=tinyplace`.
    pub roster: Vec<AgentDescriptor>,
    /// Latest presence per peer agent id.
    pub presence: HashMap<String, AgentPresence>,
    /// A problem the operator needs to know about, such as a failed pre-key
    /// publish — which leaves the identity reachable in the directory but unable
    /// to receive any DM.
    ///
    /// Carried here rather than printed: the consumers of this service own a
    /// terminal screen, and anything written to stdout or stderr under one lands
    /// on top of the UI and never clears.
    pub notice: Option<String>,
}
/// A running tiny.place background service. Dropping it aborts its loops.
pub struct TinyplaceService {
    pub(super) observation: Arc<Mutex<TinyplaceObservation>>,
    pub(super) contacts: ContactDesk,
    pub(super) transport: crate::daemon::transport::SignalTransport,
    pub(super) endpoint: String,
    pub(super) handles: Vec<JoinHandle<()>>,
}
