//! Cancelling a run in the window before its future is first polled.
//!
//! A caller that spawns the run and hands its id straight back — which is what
//! `workflow_run` does by default — creates a gap: the id is out in the world
//! while the task holding it has not been polled once. These cases pin the
//! behaviour that closes that gap, where the caller claims the id itself and
//! the run adopts the claim rather than taking out a second one.

use super::*;

use crate::workflows::run::RunGuard;

/// The claim a caller took out before handing the id back is *adopted*, not
/// treated as a competing run. Without this the run would refuse itself with
/// "already executing" — the caller's own claim reading as somebody else's.
#[tokio::test]
async fn a_claim_taken_before_the_run_starts_is_adopted() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    let claim = RunGuard::claim("run-preclaimed").expect("a fresh id is claimable");
    let mut context = harness.context(Arc::new(HangingDispatch));
    context.claim = Some(claim);

    let run = tokio::spawn(async move {
        run_workflow(
            context,
            "gated",
            "run-preclaimed",
            json!({ "approvals": ["review"] }),
            Default::default(),
        )
        .await
    });

    wait_until_dispatched("run-preclaimed").await;
    assert!(cancel("run-preclaimed"), "the adopted claim is cancellable");
    let record = run.await.unwrap().expect("settles");
    assert_eq!(record.status, RunStatus::Cancelled);
    assert!(
        !is_running("run-preclaimed"),
        "adopting the claim must not leak it: the run still deregisters on exit"
    );
}

/// A claim belongs to the id it was taken out under. Adopting one for a
/// different run would register the wrong id: `cancel` for the run actually
/// executing would answer "not found", and a second dispatch of that id could
/// start beside it. The mismatch is refused instead.
#[tokio::test]
async fn a_claim_for_another_run_is_refused_rather_than_adopted() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    let claim = RunGuard::claim("run-elsewhere").expect("a fresh id is claimable");
    let mut context = harness.context(Arc::new(HangingDispatch));
    context.claim = Some(claim);

    let err = run_workflow(
        context,
        "gated",
        "run-here",
        json!({ "approvals": ["review"] }),
        Default::default(),
    )
    .await
    .expect_err("a claim for another id cannot start this run");

    assert!(
        err.to_string().contains("run-elsewhere"),
        "the refusal names the mismatched claim: {err}"
    );
    assert!(
        !is_running("run-here"),
        "the refused run never registered its own id"
    );
}

/// The case the claim exists for: a cancel arriving *before the run future has
/// ever been polled* is both answered honestly and honoured. Calling an async
/// fn only builds the future, so nothing in this test has run the workflow yet
/// when the cancel lands — exactly the state a spawned-but-unpolled task is in.
#[tokio::test]
async fn a_cancel_landing_before_the_run_is_polled_still_stops_it() {
    let harness = Harness::new();
    harness.install(&gated(), "gated");
    let claim = RunGuard::claim("run-cancel-first").expect("a fresh id is claimable");
    let mut context = harness.context(Arc::new(HangingDispatch));
    context.claim = Some(claim);

    // Built, deliberately not awaited: the workflow has not executed a step.
    let run = run_workflow(
        context,
        "gated",
        "run-cancel-first",
        json!({ "approvals": ["review"] }),
        Default::default(),
    );

    let answer = crate::workflows::ops::cancel_run(&harness.dyn_store(), "run-cancel-first");
    assert_eq!(
        answer["cancelled"],
        json!(true),
        "the claim is what makes this findable: {answer}"
    );

    // The flag outlives the notify, so the run sees the cancel on its first
    // poll rather than waiting forever for a wake-up that already happened.
    let record = run.await.expect("settles");
    assert_eq!(record.status, RunStatus::Cancelled);
    assert!(!is_running("run-cancel-first"));
}
