//! Capacity regressions for the process-wide task registry.

use std::sync::Arc;

use async_trait::async_trait;

use super::super::super::types::{FleetOps, FleetWorker};
use super::super::registry::MAX_IN_FLIGHT_GLOBAL;
use super::super::types::{SpawnError, TaskRegistry, TaskState};
use crate::hub::{RunError, TaskOutcome, TaskRequest};

/// A fleet whose tasks remain active until the test runtime shuts down.
struct NeverSettles;

/// A fleet whose dispatch future disappears before publishing readiness.
struct PanicsBeforeStart;

#[async_trait]
impl FleetOps for NeverSettles {
    fn workers(&self) -> Option<Vec<FleetWorker>> {
        Some(Vec::new())
    }

    fn default_worker(&self) -> Option<String> {
        None
    }

    async fn dispatch(
        &self,
        _request: TaskRequest,
        _status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        std::future::pending().await
    }

    fn abort(&self, _abort_id: &str) -> bool {
        false
    }
}

#[async_trait]
impl FleetOps for PanicsBeforeStart {
    fn workers(&self) -> Option<Vec<FleetWorker>> {
        Some(Vec::new())
    }

    fn default_worker(&self) -> Option<String> {
        None
    }

    async fn dispatch(
        &self,
        _request: TaskRequest,
        _status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        panic!("dispatch setup panic")
    }

    fn abort(&self, _abort_id: &str) -> bool {
        false
    }
}

/// A unique request for one registry slot.
fn request(index: usize) -> TaskRequest {
    TaskRequest {
        task_id: format!("wire-{index}"),
        abort_id: format!("abort-{index}"),
        cycle_id: None,
        instruction: "stay active".into(),
        worker_address: "worker".into(),
        provider: None,
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    }
}

#[tokio::test]
async fn active_tasks_are_capped_across_successive_grants() {
    let registry = TaskRegistry::new();
    let fleet: Arc<dyn FleetOps> = Arc::new(NeverSettles);
    for index in 0..MAX_IN_FLIGHT_GLOBAL {
        registry
            .spawn_below_limit(
                fleet.clone(),
                format!("grant-{index}"),
                request(index),
                MAX_IN_FLIGHT_GLOBAL + 1,
            )
            .await
            .expect("a slot below the global ceiling");
    }

    let error = registry
        .spawn_below_limit(
            fleet,
            "one-more-grant".into(),
            request(MAX_IN_FLIGHT_GLOBAL),
            MAX_IN_FLIGHT_GLOBAL + 1,
        )
        .await
        .expect_err("the process-wide ceiling must hold across grants");
    assert!(matches!(
        error,
        SpawnError::GlobalAtCapacity(MAX_IN_FLIGHT_GLOBAL)
    ));
}

#[tokio::test]
async fn admitting_a_replacement_preserves_a_settled_result_with_a_waiter() {
    let registry = TaskRegistry::new();
    let fleet: Arc<dyn FleetOps> = Arc::new(NeverSettles);
    for index in 0..MAX_IN_FLIGHT_GLOBAL {
        registry
            .spawn_below_limit(
                fleet.clone(),
                "grant".into(),
                request(index),
                MAX_IN_FLIGHT_GLOBAL + 1,
            )
            .await
            .unwrap();
    }

    let _waiter = {
        let mut tasks = registry.inner.lock().unwrap();
        let tracked = tasks.get_mut("abort-0").unwrap();
        tracked.entry.state = TaskState::Done(Box::new(TaskOutcome {
            reply: "kept".into(),
            usage: crate::tinyplace::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            harness: None,
        }));
        tracked.entry.finished_at = Some(crate::clock::now_millis());
        tracked.settled.subscribe()
    };

    registry
        .spawn_below_limit(
            fleet,
            "grant".into(),
            request(MAX_IN_FLIGHT_GLOBAL),
            MAX_IN_FLIGHT_GLOBAL + 1,
        )
        .await
        .unwrap();

    assert_eq!(
        registry.get("grant", "abort-0").unwrap().state.status(),
        "done"
    );
}

#[tokio::test]
async fn dispatch_setup_failure_does_not_leak_a_running_slot() {
    let registry = TaskRegistry::new();
    let task_id = registry
        .spawn_below_limit(Arc::new(PanicsBeforeStart), "grant".into(), request(1), 2)
        .await
        .unwrap();

    let entry = registry.get("grant", &task_id).unwrap();
    assert!(entry.state.is_settled());
    assert_eq!(entry.state.status(), "failed");
}
