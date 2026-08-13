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

/// Dispatch over the real hub task runner.
pub struct TaskRunnerDispatch {
    runner: Arc<TaskRunner>,
}

impl TaskRunnerDispatch {
    /// Dispatch through `runner`.
    pub fn new(runner: Arc<TaskRunner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl HarnessDispatch for TaskRunnerDispatch {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        // No status channel: a workflow's progress is reported per *node* by the
        // run observer, and forwarding a harness's token-level chatter here as
        // well would double-report the same work.
        self.runner.run_workflow_node(request, None).await
    }

    async fn dispatch_with_status(
        &self,
        request: TaskRequest,
        status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        self.runner.run_workflow_node(request, status).await
    }

    fn abort_in_flight(&self) {
        self.runner.abort_all();
    }
}
