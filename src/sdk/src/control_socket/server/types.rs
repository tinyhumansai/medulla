//! State owned by the live control-socket server and its task registry.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use super::super::grants::{Grant, GrantRegistry};
use crate::hub::TaskOutcome;

/// How a dispatched task ended, in terms a model can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    /// Still running.
    Running,
    /// Settled with a reply.
    Done(Box<TaskOutcome>),
    /// Settled without one.
    Failed {
        /// The wire status: `failed`, `aborted`, `busy`, `held`, or `timeout`.
        status: &'static str,
        /// What went wrong, phrased for the model that has to decide what next.
        message: String,
        /// Whether the same dispatch a little later might succeed.
        retryable: bool,
    },
}

impl TaskState {
    /// The wire status string for this state.
    pub fn status(&self) -> &'static str {
        match self {
            TaskState::Running => "running",
            TaskState::Done(_) => "done",
            TaskState::Failed { status, .. } => status,
        }
    }

    /// Whether the task has settled.
    pub fn is_settled(&self) -> bool {
        !matches!(self, TaskState::Running)
    }
}

/// One dispatched task.
#[derive(Debug, Clone)]
pub struct TaskEntry {
    /// The handle returned to the caller.
    pub task_id: String,
    /// The grant that dispatched it. Scopes reads, polls, and aborts.
    pub token: String,
    /// Where it was sent.
    pub worker: String,
    /// The first line of the instruction, for a listing.
    pub instruction: String,
    /// Epoch ms the dispatch was accepted.
    pub started_at: i64,
    /// Epoch ms it settled, when it has.
    pub finished_at: Option<i64>,
    /// Where it got to.
    pub state: TaskState,
    /// The tail of the worker's status lines.
    pub status_tail: Vec<String>,
}

impl TaskEntry {
    /// How long this task has been running, or ran for.
    pub fn elapsed_ms(&self) -> i64 {
        self.finished_at.unwrap_or_else(crate::clock::now_millis) - self.started_at
    }
}

/// Internal bookkeeping for one task and its completion signal.
pub(super) struct Tracked {
    /// The task state exposed to its owning grant.
    pub(super) entry: TaskEntry,
    /// Retained completion signal used to close poller races.
    pub(super) settled: watch::Sender<bool>,
}

/// Why a task could not be recorded for dispatch.
pub(super) enum SpawnError {
    /// This grant already owns the reported number of running tasks.
    AtCapacity(usize),
    /// The shared task table is unavailable after an internal panic.
    Unavailable,
}

/// Every task this control plane has dispatched.
#[derive(Clone, Default)]
pub struct TaskRegistry {
    /// Entries by server-minted abort handle.
    pub(super) inner: Arc<Mutex<HashMap<String, Tracked>>>,
}

/// What one control-socket connection has established about itself.
///
/// A connection starts unauthenticated and stays that way until a successful
/// `hello`; no other operation is answered before then.
#[derive(Default)]
pub struct SessionState {
    /// The redeemed grant and the token it came from, once `hello` succeeded.
    pub(super) authenticated: Option<(String, Grant)>,
}

impl SessionState {
    /// The grant this connection holds, if it has completed `hello`.
    pub fn grant(&self) -> Option<&Grant> {
        self.authenticated.as_ref().map(|(_, grant)| grant)
    }
}

/// A bound control socket, serving until dropped.
///
/// Dropping stops the accept loop, closes every connection already open, and
/// unlinks the socket file — the last only if the file at that path is still the
/// one this server bound, since a later drop must not delete the socket a
/// restarted instance has since put there.
pub struct ControlServer {
    pub(super) path: PathBuf,
    /// The socket identity at bind time, used to avoid unlinking a replacement.
    pub(super) identity: Option<(u64, u64)>,
    /// The listener task serving incoming control connections.
    pub(super) accept: tokio::task::JoinHandle<()>,
    /// Capabilities minted for harnesses spawned by this server.
    pub(super) grants: GrantRegistry,
    /// Flipped on drop; watched by the accept loop and every connection.
    pub(super) shutdown: tokio::sync::watch::Sender<bool>,
}
