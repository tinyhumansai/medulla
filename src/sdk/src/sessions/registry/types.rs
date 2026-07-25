//! Data types for the `registry` module.
#[allow(unused_imports)]
use super::*;
/// What one turn should do about session continuity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnPlan {
    /// The lifetime class this turn runs under.
    pub class: SessionClass,
    /// A harness session id to resume, when one is already bound and the
    /// provider can resume it.
    pub resume_session_id: Option<String>,
    /// Whether this turn's captured session id should be recorded as the
    /// conversation's binding. True exactly on the first unbound turn — the
    /// `Bounded → Unbound` edge.
    pub bind: bool,
}
/// Insertion-ordered bindings plus the per-key turn chains.
#[derive(Default)]
pub(super) struct Inner {
    /// `map_key -> harness session id`, in least-recently-used-first order.
    pub(super) bindings: Vec<(String, String)>,
}
/// Remembers which harness session each conversation is bound to, and serializes
/// turns per conversation.
///
/// Cheap to clone (an `Arc`), so the daemon and the session manager can share one.
#[derive(Clone)]
pub struct SessionRegistry {
    pub(super) inner: Arc<Mutex<Inner>>,
    /// One async mutex per conversation key, held for the duration of a turn.
    /// Only unbound turns take one; bounded turns run concurrently by design.
    pub(super) chains: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
    pub(super) max_bindings: usize,
}
