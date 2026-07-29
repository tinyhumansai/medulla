//! Contract tests for narrow runtime capability adapters.

use super::{FleetCapability, SteeringCapability, UsageCapability};
use crate::runtime::mock::MockRuntime;
use crate::runtime::WorkerOp;

/// Read workers through only the fleet capability bound.
fn worker_count(runtime: &(impl FleetCapability + ?Sized)) -> usize {
    runtime.workers().len()
}

#[test]
fn blanket_adapter_exposes_narrow_capabilities() {
    let runtime = MockRuntime::empty();

    SteeringCapability::answer_question(&runtime, "cycle".into(), "question".into(), "yes".into());
    SteeringCapability::cancel_task(&runtime, "cycle".into(), "task".into());
    assert_eq!(worker_count(&runtime), 0);
    assert!(FleetCapability::worker_activity(&runtime).is_empty());
    assert!(FleetCapability::stream_state(&runtime).is_none());

    futures::executor::block_on(async {
        assert!(UsageCapability::team_usage(&runtime)
            .await
            .unwrap()
            .is_none());
        FleetCapability::worker_op(
            &runtime,
            WorkerOp::Select {
                id: "worker".into(),
            },
        )
        .await
        .unwrap();
    });
}
