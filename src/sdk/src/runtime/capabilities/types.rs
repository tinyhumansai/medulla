//! Capability traits and their compatibility adapters.

use futures::future::BoxFuture;

use crate::hub::WorkerActivity;

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

/// Composite bound for code that intentionally consumes every optional runtime
/// capability while remaining independent of chat/session lifecycle methods.
pub trait RuntimeCapabilities: UsageCapability + SteeringCapability + FleetCapability {}

impl<T> RuntimeCapabilities for T where
    T: UsageCapability + SteeringCapability + FleetCapability + ?Sized
{
}
