//! Contract tests for narrow runtime capability adapters.

use super::{
    FeedbackCapability, FleetCapability, MemoryCapability, SteeringCapability, UsageCapability,
};
use crate::client::{FeedbackQuery, FeedbackType};
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
    assert!(MemoryCapability::memory_status(&runtime).is_none());
    assert!(MemoryCapability::memory_search(&runtime, "query".into(), None, 5).is_empty());
    assert!(MemoryCapability::memory_directives(&runtime).is_empty());

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

        let page = FeedbackCapability::list_feedback(&runtime, FeedbackQuery::default())
            .await
            .unwrap()
            .unwrap();
        let id = page.items[0].id.clone();
        FeedbackCapability::feedback_detail(&runtime, id.clone())
            .await
            .unwrap();
        FeedbackCapability::vote_feedback(&runtime, id.clone(), 1)
            .await
            .unwrap();
        FeedbackCapability::comment_feedback(&runtime, id, "useful".into())
            .await
            .unwrap();
        FeedbackCapability::submit_feedback(
            &runtime,
            FeedbackType::Bug,
            "Unexpected behavior".into(),
            "Steps to reproduce".into(),
        )
        .await
        .unwrap();
    });
}
