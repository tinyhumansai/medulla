//! Approval-gate pause and resume behavior.

use super::*;

#[tokio::test]
async fn an_approval_gate_pauses_the_run_and_names_what_it_is_waiting_on() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");

    let record = run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "gated",
        "run-3",
        json!({}),
    )
    .await
    .expect("runs");

    assert_eq!(record.status, RunStatus::PendingApproval);
    assert_eq!(record.pending_approvals, vec!["review".to_string()]);
    assert!(
        record
            .summary
            .as_deref()
            .is_some_and(|summary| summary.contains("awaiting approval: review")),
        "the persisted summary must describe the resumable state: {:?}",
        record.summary
    );
    assert!(!record.status.is_settled(), "a gate is resumable");
}

#[tokio::test]
async fn approving_the_gate_resumes_the_run_to_completion() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "gated",
        "run-4",
        json!({}),
    )
    .await
    .unwrap();

    let resumed = resume_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "run-4",
        vec!["review".into()],
        Vec::new(),
    )
    .await
    .expect("resumes");

    assert_eq!(resumed.status, RunStatus::Succeeded);
    assert!(resumed.pending_approvals.is_empty());
}

#[tokio::test]
async fn a_resume_naming_no_pending_gate_is_refused() {
    // The engine treats the resume call itself as consent, so without this
    // check any resume would release every gate the run is holding.
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "gated",
        "run-5",
        json!({}),
    )
    .await
    .unwrap();

    let err = resume_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "run-5",
        vec!["some-other-node".into()],
        Vec::new(),
    )
    .await
    .expect_err("must not release the gate");

    assert!(
        err.to_string().contains("review"),
        "say which gate is actually waiting: {err}"
    );
    assert_eq!(
        require_run(harness.store.as_ref(), "run-5").unwrap().status,
        RunStatus::PendingApproval,
        "the run must still be parked"
    );
}

#[tokio::test]
async fn resuming_a_run_that_is_not_waiting_is_refused() {
    let harness = Harness::new();
    harness.install(&diamond(), "diamond");
    run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "diamond",
        "run-6",
        json!({}),
    )
    .await
    .unwrap();

    let err = resume_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "run-6",
        vec!["anything".into()],
        Vec::new(),
    )
    .await
    .expect_err("already finished");

    assert!(
        err.to_string().contains("not awaiting approval"),
        "got {err}"
    );
}

#[tokio::test]
async fn a_resume_may_reject_the_only_gate_without_approving_anything() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "gated",
        "run-reject",
        json!({}),
    )
    .await
    .unwrap();

    // Rejecting is a legitimate way to settle a run; requiring an approval
    // would make --reject unusable on a single-gate workflow.
    let settled = resume_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "run-reject",
        Vec::new(),
        vec!["review".into()],
    )
    .await
    .expect("a rejection is a decision");

    assert!(settled.status.is_settled(), "got {:?}", settled.status);
}

#[tokio::test]
async fn a_resumed_timeout_keeps_errors_swallowed_before_the_timeout() {
    let harness = Harness::new();
    harness.install(&gated_error_then_hang(), "gated-error-then-hang");
    run_workflow(
        harness.context(Arc::new(StubDispatch::default())),
        "gated-error-then-hang",
        "run-resume-timeout",
        json!({}),
    )
    .await
    .expect("pauses");

    let mut context = harness.context(Arc::new(ErrorThenHangDispatch));
    let mut impatient = (*harness.settings).clone();
    impatient.run_timeout_secs = 1;
    context.settings = Arc::new(impatient);
    let record = resume_workflow(
        context,
        "run-resume-timeout",
        vec!["review".into()],
        Vec::new(),
    )
    .await
    .expect("timeout is recorded");

    assert_eq!(record.status, RunStatus::Failed);
    assert!(
        record.diagnosis.as_ref().is_some_and(|diagnosis| diagnosis
            .hidden_errors
            .iter()
            .any(|error| error.node_id == "swallowed")),
        "the host timeout must not reclassify the earlier swallowed error: {:?}",
        record.diagnosis
    );
}
