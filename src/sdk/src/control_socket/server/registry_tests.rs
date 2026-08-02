//! Capacity regressions for the process-wide task registry.

use std::sync::Arc;

use async_trait::async_trait;

use super::super::types::{FleetOps, FleetWorker};
use super::registry::MAX_ENTRIES;
use super::types::{SpawnError, TaskRegistry};
use crate::hub::{RunError, TaskOutcome, TaskRequest};

/// A fleet whose tasks remain active until the test runtime shuts down.
struct NeverSettles;

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
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    }
}

#[tokio::test]
async fn active_tasks_are_capped_across_successive_grants() {
    let registry = TaskRegistry::new();
    let fleet: Arc<dyn FleetOps> = Arc::new(NeverSettles);
    for index in 0..MAX_ENTRIES {
        registry
            .spawn_below_limit(
                fleet.clone(),
                format!("grant-{index}"),
                request(index),
                MAX_ENTRIES + 1,
            )
            .await
            .expect("a slot below the global ceiling");
    }

    let error = registry
        .spawn_below_limit(
            fleet,
            "one-more-grant".into(),
            request(MAX_ENTRIES),
            MAX_ENTRIES + 1,
        )
        .await
        .expect_err("the process-wide ceiling must hold across grants");
    assert!(matches!(error, SpawnError::GlobalAtCapacity(MAX_ENTRIES)));
}
