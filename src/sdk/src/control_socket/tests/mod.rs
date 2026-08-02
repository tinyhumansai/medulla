//! Unit tests for the control plane, plus the fake fleet they share.
//!
//! Split by what each file pins down: [`protocol`] covers the request handler's
//! branches, [`grants`] covers the capability model that makes this surface
//! exclusive, [`paths`] covers socket resolution and bind safety, and
//! [`roundtrip`] drives a real listener with a real client.

#[cfg(unix)]
mod grants;
mod paths;
#[cfg(unix)]
mod protocol;
#[cfg(unix)]
mod roundtrip;

#[cfg(unix)]
use std::sync::Mutex;

#[cfg(unix)]
use tokio::sync::mpsc;

#[cfg(unix)]
use crate::hub::{RunError, TaskOutcome, TaskRequest};
#[cfg(unix)]
use crate::tinyplace::TokenUsage;

#[cfg(unix)]
use super::types::{FleetOps, FleetWorker};

/// How a fake dispatch should end.
#[cfg(unix)]
#[derive(Debug, Clone)]
pub(super) enum FakeOutcome {
    /// Settle immediately with this reply.
    Reply(String),
    /// Settle immediately with this failure.
    Fail(RunError),
    /// Never settle on its own; only an abort ends it.
    Hang,
}

/// A fleet that answers however a test tells it to.
#[cfg(unix)]
pub(super) struct FakeFleet {
    /// The roster, or `None` to simulate a hub that has not connected.
    workers: Mutex<Option<Vec<FleetWorker>>>,
    default_worker: Mutex<Option<String>>,
    outcome: Mutex<FakeOutcome>,
    /// Every request that reached `dispatch`, for asserting on what was built.
    pub(super) dispatched: Mutex<Vec<TaskRequest>>,
    /// Every abort id that reached `abort`.
    pub(super) aborted: Mutex<Vec<String>>,
    /// Set when a hanging dispatch is aborted.
    ///
    /// A [`watch`](tokio::sync::watch) rather than a `Notify` for the same
    /// reason the registry uses one: an abort can land before the spawned
    /// dispatch has reached its await, and `notify_waiters` would fire into an
    /// empty waiter set and be lost. A watch retains the value, so the ordering
    /// stops mattering — otherwise these tests pass or fail on scheduling luck.
    hang_release: tokio::sync::watch::Sender<bool>,
}

#[cfg(unix)]
impl FakeFleet {
    /// A fleet with one worker that replies immediately.
    pub(super) fn new() -> Self {
        FakeFleet {
            workers: Mutex::new(Some(vec![FleetWorker {
                id: "alpha".into(),
                address: "alpha-address".into(),
                harness: "claude".into(),
                selected: true,
                ..FleetWorker::default()
            }])),
            default_worker: Mutex::new(Some("alpha-address".into())),
            outcome: Mutex::new(FakeOutcome::Reply("done".into())),
            dispatched: Mutex::new(Vec::new()),
            aborted: Mutex::new(Vec::new()),
            hang_release: tokio::sync::watch::channel(false).0,
        }
    }

    /// A fleet whose hub has not connected yet.
    pub(super) fn unconnected() -> Self {
        let fleet = Self::new();
        *fleet.workers.lock().unwrap() = None;
        *fleet.default_worker.lock().unwrap() = None;
        fleet
    }

    /// Replace the roster.
    pub(super) fn with_workers(self, workers: Vec<FleetWorker>) -> Self {
        *self.workers.lock().unwrap() = Some(workers);
        self
    }

    /// Choose how the next dispatches settle.
    pub(super) fn with_outcome(self, outcome: FakeOutcome) -> Self {
        *self.outcome.lock().unwrap() = outcome;
        self
    }
}

#[cfg(unix)]
#[async_trait::async_trait]
impl FleetOps for FakeFleet {
    fn workers(&self) -> Option<Vec<FleetWorker>> {
        self.workers.lock().unwrap().clone()
    }

    fn default_worker(&self) -> Option<String> {
        self.default_worker.lock().unwrap().clone()
    }

    async fn dispatch(
        &self,
        request: TaskRequest,
        _status: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        let outcome = self.outcome.lock().unwrap().clone();
        self.dispatched.lock().unwrap().push(request);
        match outcome {
            FakeOutcome::Reply(reply) => Ok(TaskOutcome {
                reply,
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                },
                harness: None,
            }),
            FakeOutcome::Fail(error) => Err(error),
            FakeOutcome::Hang => {
                let mut released = self.hang_release.subscribe();
                let _ = released.wait_for(|released| *released).await;
                Err(RunError::Aborted)
            }
        }
    }

    fn abort(&self, abort_id: &str) -> bool {
        self.aborted.lock().unwrap().push(abort_id.to_string());
        self.hang_release.send_replace(true);
        matches!(*self.outcome.lock().unwrap(), FakeOutcome::Hang)
    }
}
