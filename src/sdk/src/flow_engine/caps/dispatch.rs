//! Handing a workflow node's instruction to a harness.
//!
//! This is the one Medulla-side abstraction in the seam, and it earns its place
//! twice over: it keeps the agent adapter from depending on a live
//! [`TaskRunner`] (which needs a relay, a contact, and a peer that answers), and
//! it is what lets a test assert *what was dispatched* rather than only what
//! came back.
//!
//! Everything else in this seam implements a trait the engine already defines.

use std::sync::Arc;

use async_trait::async_trait;

use crate::hub::{RunError, TaskOutcome, TaskRequest, TaskRunner};

/// Somewhere a workflow node's instruction can be run.
#[async_trait]
pub trait HarnessDispatch: Send + Sync {
    /// Run `request` to completion, returning the worker's reply.
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError>;

    /// Run `request`, forwarding the worker's progress frames to `status`.
    ///
    /// Defaulted to plain [`dispatch`](Self::dispatch) because a workflow *node*
    /// has no use for them — its progress is reported per node by the run
    /// observer, and forwarding token-level chatter would double-report the same
    /// work. The copilot is the caller that does want them: its whole pane is
    /// one turn, and a turn that takes a minute in silence reads as a hang.
    ///
    /// A dropped receiver is normal, not an error — the caller stopped watching.
    async fn dispatch_with_status(
        &self,
        request: TaskRequest,
        status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        let _ = status;
        self.dispatch(request).await
    }

    /// The harness `request` would actually run on, when this handle knows.
    ///
    /// A request names the harness a node *asked* for, and a dispatch is free to
    /// substitute: a worker that does not offer the named provider falls back to
    /// its default rather than failing a portable graph. A run inspector joining
    /// on the recorded harness would then name something that is not executing,
    /// so the dispatch that performs the substitution gets to answer for it.
    ///
    /// Defaulted to `None`, meaning "no better answer than the request itself" —
    /// a dispatch that substitutes nothing has nothing to correct.
    fn effective_harness(&self, request: &TaskRequest) -> Option<String> {
        let _ = request;
        None
    }

    /// Stop every dispatch this handle currently has in flight.
    ///
    /// Deliberately not per-task. The caller that wants this is the copilot
    /// pane, which holds a dispatch of its own serving one conversation with at
    /// most one turn running — so "stop what this handle is doing" is exactly
    /// what the operator means by pressing the key, and it needs no task id
    /// threaded back out through the turn that generated it.
    ///
    /// Defaulted to a no-op because not every dispatch can be stopped: a
    /// stand-in has nothing in flight, and a handle without a live runner has
    /// nothing to signal.
    fn abort_in_flight(&self) {}
}

/// What a dispatch handle is running: a graph's steps, or the copilot.
///
/// The same loopback host serves both, and both are workflow-plane work — but
/// they want opposite tool surfaces. A graph step is withheld the `workflow_*`
/// family so it cannot start the graph it is a step of; the copilot exists to
/// call `workflow_create` and the editing verbs and would be unable to do its
/// job without them.
///
/// Carried on the *handle* rather than read off the request, so the widening
/// choice belongs to whoever built the dispatch — the copilot's own
/// construction site — and a node's request cannot ask for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchRole {
    /// A step of a running graph. The default, and the withheld one.
    Node,
    /// A copilot authoring turn, which keeps the workflow tools.
    Authoring,
}

/// Dispatch over the real hub task runner.
pub struct TaskRunnerDispatch {
    runner: Arc<TaskRunner>,
    role: DispatchRole,
}

impl TaskRunnerDispatch {
    /// Dispatch `agent`-node instructions through `runner`.
    pub fn new(runner: Arc<TaskRunner>) -> Self {
        Self {
            runner,
            role: DispatchRole::Node,
        }
    }

    /// Dispatch *authoring* turns through `runner`.
    ///
    /// What the workflow copilot holds. Everything else about the dispatch is
    /// identical; the difference is the marker on the frame, which is what
    /// keeps the workflow tools with a turn whose whole purpose is to call
    /// them.
    pub fn authoring(runner: Arc<TaskRunner>) -> Self {
        Self {
            runner,
            role: DispatchRole::Authoring,
        }
    }
}

#[async_trait]
impl HarnessDispatch for TaskRunnerDispatch {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        // No status channel: a workflow's progress is reported per *node* by the
        // run observer, and forwarding a harness's token-level chatter here as
        // well would double-report the same work.
        self.dispatch_with_status(request, None).await
    }

    async fn dispatch_with_status(
        &self,
        request: TaskRequest,
        status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        match self.role {
            DispatchRole::Node => self.runner.run_workflow_node(request, status).await,
            DispatchRole::Authoring => self.runner.run_workflow_authoring(request, status).await,
        }
    }

    fn abort_in_flight(&self) {
        self.runner.abort_all();
    }
}
