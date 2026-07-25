//! Capability traits and their compatibility adapters.

use futures::future::BoxFuture;

use crate::client::{
    FeedbackComment, FeedbackDetail, FeedbackItem, FeedbackPage, FeedbackQuery, FeedbackSubmission,
    FeedbackType,
};
use crate::hub::WorkerActivity;
use crate::memory::{MemoryHit, MemoryStatus};

use super::super::{Runtime, StreamState, WorkerInfo, WorkerOp};

/// Account-usage reads exposed by a runtime with a backend account.
pub trait UsageCapability: Send + Sync {
    /// Return account-level usage, or `None` when the adapter has no account.
    fn team_usage(&self) -> BoxFuture<'static, anyhow::Result<Option<serde_json::Value>>>;
}

impl<T: Runtime + ?Sized> UsageCapability for T {
    fn team_usage(&self) -> BoxFuture<'static, anyhow::Result<Option<serde_json::Value>>> {
        Runtime::team_usage(self)
    }
}

/// Operator controls for questions and running task lanes.
pub trait SteeringCapability: Send + Sync {
    /// Answer one pending runtime question.
    fn answer_question(&self, cycle_id: String, question_id: String, body: String);

    /// Cancel one running task.
    fn cancel_task(&self, cycle_id: String, task_id: String);
}

impl<T: Runtime + ?Sized> SteeringCapability for T {
    fn answer_question(&self, cycle_id: String, question_id: String, body: String) {
        Runtime::answer_question(self, cycle_id, question_id, body);
    }

    fn cancel_task(&self, cycle_id: String, task_id: String) {
        Runtime::cancel_task(self, cycle_id, task_id);
    }
}

/// Worker registry, worker activity, and stream-health operations.
pub trait FleetCapability: Send + Sync {
    /// Return current work delegated to managed workers.
    fn worker_activity(&self) -> Vec<WorkerActivity>;

    /// Return the managed worker registry.
    fn workers(&self) -> Vec<WorkerInfo>;

    /// Apply a worker-registry mutation.
    fn worker_op(&self, op: WorkerOp) -> BoxFuture<'static, anyhow::Result<()>>;

    /// Return event-stream health when the runtime tracks a lossy stream.
    fn stream_state(&self) -> Option<StreamState>;
}

impl<T: Runtime + ?Sized> FleetCapability for T {
    fn worker_activity(&self) -> Vec<WorkerActivity> {
        Runtime::worker_activity(self)
    }

    fn workers(&self) -> Vec<WorkerInfo> {
        Runtime::workers(self)
    }

    fn worker_op(&self, op: WorkerOp) -> BoxFuture<'static, anyhow::Result<()>> {
        Runtime::worker_op(self, op)
    }

    fn stream_state(&self) -> Option<StreamState> {
        Runtime::stream_state(self)
    }
}

/// Read-only access to an attached persona-memory service.
pub trait MemoryCapability: Send + Sync {
    /// Return memory health, or `None` when memory is not attached.
    fn memory_status(&self) -> Option<MemoryStatus>;

    /// Search memory using an optional facet and maximum result count.
    fn memory_search(&self, query: String, facet: Option<String>, k: usize) -> Vec<MemoryHit>;

    /// Return the attached persona's verbatim directives.
    fn memory_directives(&self) -> Vec<String>;
}

impl<T: Runtime + ?Sized> MemoryCapability for T {
    fn memory_status(&self) -> Option<MemoryStatus> {
        Runtime::memory_status(self)
    }

    fn memory_search(&self, query: String, facet: Option<String>, k: usize) -> Vec<MemoryHit> {
        Runtime::memory_search(self, query, facet, k)
    }

    fn memory_directives(&self) -> Vec<String> {
        Runtime::memory_directives(self)
    }
}

/// Public feedback-board reads and mutations.
pub trait FeedbackCapability: Send + Sync {
    /// Return one board page, or `None` when no feedback backend is attached.
    fn list_feedback(
        &self,
        query: FeedbackQuery,
    ) -> BoxFuture<'static, anyhow::Result<Option<FeedbackPage>>>;

    /// Return one feedback item and its comments.
    fn feedback_detail(&self, id: String) -> BoxFuture<'static, anyhow::Result<FeedbackDetail>>;

    /// Cast, change, or retract a vote.
    fn vote_feedback(
        &self,
        id: String,
        value: i8,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackItem>>;

    /// Add a comment to one feedback item.
    fn comment_feedback(
        &self,
        id: String,
        body: String,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackComment>>;

    /// Submit a new feedback item for moderation.
    fn submit_feedback(
        &self,
        kind: FeedbackType,
        title: String,
        body: String,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackSubmission>>;
}

impl<T: Runtime + ?Sized> FeedbackCapability for T {
    fn list_feedback(
        &self,
        query: FeedbackQuery,
    ) -> BoxFuture<'static, anyhow::Result<Option<FeedbackPage>>> {
        Runtime::list_feedback(self, query)
    }

    fn feedback_detail(&self, id: String) -> BoxFuture<'static, anyhow::Result<FeedbackDetail>> {
        Runtime::feedback_detail(self, id)
    }

    fn vote_feedback(
        &self,
        id: String,
        value: i8,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackItem>> {
        Runtime::vote_feedback(self, id, value)
    }

    fn comment_feedback(
        &self,
        id: String,
        body: String,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackComment>> {
        Runtime::comment_feedback(self, id, body)
    }

    fn submit_feedback(
        &self,
        kind: FeedbackType,
        title: String,
        body: String,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackSubmission>> {
        Runtime::submit_feedback(self, kind, title, body)
    }
}

/// Composite bound for code that intentionally consumes every optional runtime
/// capability while remaining independent of chat/session lifecycle methods.
pub trait RuntimeCapabilities:
    UsageCapability + SteeringCapability + FleetCapability + MemoryCapability + FeedbackCapability
{
}

impl<T> RuntimeCapabilities for T where
    T: UsageCapability
        + SteeringCapability
        + FleetCapability
        + MemoryCapability
        + FeedbackCapability
        + ?Sized
{
}
