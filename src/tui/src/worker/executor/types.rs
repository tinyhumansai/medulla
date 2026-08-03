//! Data types for the `executor` module.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::super::pty::{LaunchSpec, PtyManager};

/// Checkout and branch last reported by a reusable harness session.
pub(super) type WorkspaceContext = (Option<String>, Option<String>, Option<String>);
/// The session a task will run in, and whether it was already running.
///
/// The distinction decides how its transcript is tailed: a fresh session writes
/// a new file, a reused one is appending to a file that already holds answered
/// turns.
pub(super) struct OpenedSession {
    /// The manager's local session id.
    pub(super) id: String,
    /// The harness's own session id, once known.
    pub(super) harness_session_id: Option<String>,
    /// Whether this task joined a session that was already running.
    pub(super) reused: bool,
    /// Whether the already-running child has a GitHub repository override.
    pub(super) gh_repo_is_set: bool,
}
/// What serving a task needs: an existing session, or a launch to perform.
///
/// The split exists so the decision and the launch can happen on different
/// sides of an await. `RunTaskOptions` is `Send` but not `Sync`, so a borrow of
/// it held across one would make the whole run future un-spawnable — and the
/// launch has to cross an await, because it belongs on the blocking pool rather
/// than on a runtime worker.
pub(super) enum SessionPlan {
    /// An idle session for this conversation, already claimed.
    Reuse(OpenedSession),
    /// Nothing reusable: start a harness with this spec.
    Launch(LaunchSpec),
}

/// Runs delegated tasks inside live harness sessions.
#[derive(Clone)]
pub struct PtySessionExecutor {
    pub(super) sessions: PtyManager,
    pub(super) env: HashMap<String, String>,
    pub(super) workspace: String,
    /// Transcripts already latched onto, shared by every tailer this executor
    /// builds. Two concurrent tasks open two sessions in one workspace, and a
    /// codex session mints its own id, so neither tailer has anything to pin to
    /// — both match the first rollout to appear and both settle on it, handing
    /// one task's answer to two peers. The claim makes the first one exclusive.
    ///
    /// KNOWN GAP (codex only): exclusivity is not ownership. The claim stops two
    /// tailers latching the *same* transcript, but not each latching the *other's*
    /// — under concurrency a tailer can claim a sibling's rollout, so the peers
    /// get swapped (not duplicated) answers. Claude is unaffected: it launches
    /// with a minted `--session-id`, so its tailer pins by identity and ignores
    /// this set entirely (see `pinned_tailers_latch_by_identity_never_swapping`).
    /// The fix for codex is to bind each tailer to its own launch — most cleanly
    /// a per-session transcript directory — but that turns on how codex resolves
    /// its rollout location, which is unverified while codex is unused. Left as a
    /// documented gap rather than a guessed fix.
    pub(super) claims: Arc<Mutex<HashSet<PathBuf>>>,
    /// Checkout context retained across turns in each reusable PTY session.
    pub(super) workspace_context: Arc<Mutex<HashMap<String, WorkspaceContext>>>,
}
