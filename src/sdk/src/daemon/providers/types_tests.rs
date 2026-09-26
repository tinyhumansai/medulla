//! Unit tests for [`RunTaskOrigin::has_operator_in_the_loop`].
//!
//! `types.rs` is a single-file leaf module, so its tests live in this sibling
//! `types_tests.rs` per this repository's test-layout convention, declared
//! from `types.rs` itself rather than from the directory's `mod.rs` — the
//! same pattern `local/router.rs` uses for `router_tests.rs`. Kept out of the
//! directory-level `tests.rs`, which is already over this repository's
//! 500-line ceiling and covers a different surface (detection, argv, the run
//! helpers).
//!
//! The answer decides whether a surface holds a finished harness session open
//! for somebody to read — see the worker executor's `retains_finished_session`.
//! Getting it wrong in one direction strands a live process nobody will ever
//! look at; in the other it destroys the pane an operator went looking for.

use super::RunTaskOrigin;

#[test]
fn work_somebody_asked_for_has_an_operator_in_the_loop() {
    // Not "a human typed it": an orchestrator relaying a delegated task is
    // acting for somebody, and that somebody can attach to the session.
    assert!(RunTaskOrigin::DelegatedTask.has_operator_in_the_loop());
    assert!(RunTaskOrigin::Conversation.has_operator_in_the_loop());
    assert!(RunTaskOrigin::Interactive.has_operator_in_the_loop());
}

#[test]
fn a_workflow_node_and_a_capability_probe_have_nobody_in_the_loop() {
    // A graph node runs unattended by construction, and a probe is Medulla
    // asking this machine about itself on no peer's behalf — the same reasoning
    // that already denies the probe the operator's hooks.
    assert!(!RunTaskOrigin::Workflow.has_operator_in_the_loop());
    assert!(!RunTaskOrigin::CapabilityProbe.has_operator_in_the_loop());
}

#[test]
fn a_detached_evolution_review_has_nobody_in_the_loop_either() {
    // Spawned after a workflow run fails, restricted to proposal tools like an
    // ordinary `DelegatedTask` — but detached: nobody awaits it, and what it
    // produces lands in the evolution store for an operator to find later, not
    // in front of one now. Conflating the two would retain its session forever,
    // the same leak `Workflow` and `CapabilityProbe` were narrowed to close.
    assert!(!RunTaskOrigin::UnattendedReview.has_operator_in_the_loop());
}
