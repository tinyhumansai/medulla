//! Overlapping operator and automatic-review copilot turns.

use crate::ui::app::types::Cmd;
use crate::ui::composer;

use super::tests::{app_with, diamond};

#[test]
fn automatic_review_does_not_clear_or_drain_a_busy_operator_turn() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    app.wf.draft = composer::insert_at("", 0, "edit it");
    app.submit_copilot().expect("operator turn");
    app.copilot_started("sweep", "review the failed run");
    app.wf.draft = composer::insert_at("", 0, "then document it");
    assert!(app.submit_copilot().is_none(), "follow-up queues");

    assert!(app
        .copilot_finished("sweep", "edit done".into(), Vec::new(), None)
        .is_none());
    assert!(app.copilot_busy(), "the automatic review is still running");
    assert!(matches!(
        app.copilot_finished("sweep", "review done".into(), Vec::new(), None),
        Some(Cmd::CopilotTurn { .. })
    ));
}

#[test]
fn an_overlapping_failure_keeps_its_own_retry_and_the_queued_follow_up() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    app.wf.draft = composer::insert_at("", 0, "edit it");
    app.submit_copilot().expect("operator turn");
    app.copilot_started("sweep", "review the failed run");
    app.wf.draft = composer::insert_at("", 0, "then document it");
    assert!(app.submit_copilot().is_none(), "follow-up queues");

    app.copilot_failed("sweep", "edit it".into(), "operator turn failed".into());

    let thread = app.copilot().expect("thread");
    assert!(thread.busy, "the automatic review is still running");
    assert_eq!(thread.last_failed.as_deref(), Some("edit it"));
    assert_eq!(thread.queued.as_deref(), Some("then document it"));
    assert!(app.retry_copilot().is_none(), "retry waits for the review");
    assert!(matches!(
        app.copilot_finished("sweep", "review done".into(), Vec::new(), None),
        Some(Cmd::CopilotTurn {
            instruction,
            ..
        }) if instruction == "then document it"
    ));
}
