//! Data types for the `registry` module.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Mutex as AsyncMutex;

use crate::sessions::{SessionClass, SessionOrigin};
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
    /// Who started the bound session, and what a person named it.
    ///
    /// Carried the same way as [`TurnPlan::workspace_context`]: it belongs to
    /// the session the turn resumes, so it must survive the gap between two
    /// turns even though nothing about *this* turn produced it.
    pub identity: SessionIdentity,
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
/// Who a session belongs to and what it is called — the identity half of a
/// session, kept apart from its positional state ([`WorkspaceContext`]).
///
/// Both fields are set once, by the path that created the session:
/// [`SessionOrigin::User`] with the name the person typed, or
/// [`SessionOrigin::Orchestrator`] with no name at all. Nothing later in the
/// session's life rewrites the origin — control moves instead, and control is a
/// different field on a different type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIdentity {
    /// Who started the session. Immutable for its whole life.
    pub origin: SessionOrigin,
    /// The name a person gave it, when a person started it.
    pub name: Option<String>,
}

impl Default for SessionIdentity {
    /// The auto-creation default: started by a dispatch, unnamed.
    ///
    /// A session nobody asked for by hand is one the orchestrator made to serve
    /// a task (§4.1), and only a person supplies a name.
    fn default() -> Self {
        SessionIdentity {
            origin: SessionOrigin::Orchestrator,
            name: None,
        }
    }
}

impl SessionIdentity {
    /// An orchestrator-originated, unnamed identity.
    pub fn orchestrator() -> Self {
        SessionIdentity::default()
    }

    /// A user-originated identity, optionally carrying the name they gave it.
    pub fn user(name: Option<String>) -> Self {
        SessionIdentity {
            origin: SessionOrigin::User,
            name,
        }
    }
}

/// A resumable harness session, the workspace state accumulated within it, and
/// who it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionBinding {
    pub(super) session_id: String,
    pub(super) workspace_context: WorkspaceContext,
    pub(super) identity: SessionIdentity,
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
