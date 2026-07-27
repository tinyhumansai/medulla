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
        self.runner.run(request, None).await
    }
}
