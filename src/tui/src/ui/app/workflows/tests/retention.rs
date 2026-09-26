//! How much run history the rail lists under one workflow.
//!
//! The store keeps everything it wrote; the page shows a bounded window of it,
//! newest first, so a nightly workflow's rail stays a summary rather than a log.

use super::{app_with, diamond};

/// Persist `count` settled runs of `workflow`, oldest first.
fn persist_runs(app: &super::App, workflow: &str, count: u64) {
    let store = app.workflow_store();
    for index in 0..count {
        let mut record =
            medulla::workflows::new_run_record(&format!("run-{index}"), workflow, index + 1);
        record.status = medulla::workflows::RunStatus::Succeeded;
        record.finished_at = Some(index + 2);
        store.record_run(&record).expect("record the run");
    }
}

#[test]
fn the_rail_lists_at_most_the_configured_number_of_runs() {
    let (_home, mut app) = app_with(&[diamond("review")]);
    persist_runs(&app, "review", 40);

    app.reload_workflow_runs();

    let listed = app.workflow_runs();
    assert_eq!(
        listed.len(),
        medulla::config::WorkflowsConfig::DEFAULT_MAX_LISTED_RUNS,
        "the default cap bounds the listing"
    );
    assert_eq!(
        listed.first().map(|run| run.id.as_str()),
        Some("run-39"),
        "the newest run leads: the cap drops the tail, not the head"
    );
    assert_eq!(
        app.workflow_store()
            .list_runs("review")
            .expect("history reads")
            .len(),
        40,
        "the durable history itself is untouched"
    );
}

#[test]
fn the_cap_follows_the_configured_value() {
    let (_home, mut app) = app_with(&[diamond("review")]);
    persist_runs(&app, "review", 10);

    app.loaded.config.workflows.max_listed_runs = 3;
    app.reload_workflow_runs();

    assert_eq!(app.workflow_runs().len(), 3);
}

#[test]
fn a_cap_of_zero_still_lists_one_run() {
    // A workflow that has run must never look like one that never has.
    let (_home, mut app) = app_with(&[diamond("review")]);
    persist_runs(&app, "review", 4);

    app.loaded.config.workflows.max_listed_runs = 0;
    app.reload_workflow_runs();

    assert_eq!(app.workflow_runs().len(), 1);
}
