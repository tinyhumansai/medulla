//! Data types for the `registry` module.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Mutex as AsyncMutex;

use crate::sessions::SessionClass;
/// What one turn should do about session continuity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnPlan {
    /// The lifetime class this turn runs under.
    pub class: SessionClass,
    /// A harness session id to resume, when one is already bound and the
    /// provider can resume it.
    pub resume_session_id: Option<String>,
    /// Workspace state captured by the mapper for the bound session.
    pub workspace_context: WorkspaceContext,
    /// Whether this turn's captured session id should be recorded as the
    /// conversation's binding. True exactly on the first unbound turn — the
    /// `Bounded → Unbound` edge.
    pub bind: bool,
}
/// Workspace identity needed to interpret repository-scoped harness commands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceContext {
    /// Most recently observed working directory.
    pub cwd: Option<String>,
    /// Most recently observed branch name.
    pub branch: Option<String>,
    /// Most recently observed pull-request URL.
    pub pull_request: Option<String>,
}
/// A resumable harness session and the workspace state accumulated within it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionBinding {
    pub(super) session_id: String,
    pub(super) workspace_context: WorkspaceContext,
}
/// Insertion-ordered bindings plus the per-key turn chains.
#[derive(Default)]
pub(super) struct Inner {
    /// `map_key -> harness session id`, in least-recently-used-first order.
    pub(super) bindings: Vec<(String, SessionBinding)>,
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
