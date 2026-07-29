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
