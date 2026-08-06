//! Jumping from the Agents rail into a workflow run.
//!
//! The rail lists runs a granted harness reported over the control socket, and
//! selecting one lands here. The two cases that matter are a run whose record
//! is already on disk and one that is still executing — a run started over MCP
//! is reported long before it settles, and the record is written when it does.

use medulla::workflows::WorkflowStore;

use super::{app_with, diamond};

/// Persist a settled run of `workflow` under `id`, as a finished run leaves it.
fn persist_run(app: &super::App, workflow: &str, id: &str) {
    let store = app.workflow_store().expect("a store is configured");
    let mut record = medulla::workflows::new_run_record(id, workflow, 1);
    record.status = medulla::workflows::RunStatus::Succeeded;
    record.finished_at = Some(2);
    store.record_run(&record).expect("record the run");
}

#[test]
fn opening_a_persisted_run_selects_its_row_and_overlays_it() {
    let (_home, mut app) = app_with(&[diamond("review"), diamond("release")]);
    persist_run(&app, "review", "run-1");
    // Start on the other workflow, so the jump has to move the selection.
    app.select_workflow(1);

    app.open_workflow_run("review", "run-1");

    assert_eq!(app.tab_index, crate::ui::app::types::tab_pos("Workflows"));
    assert_eq!(app.selected_workflow().map(|w| w.id.as_str()), Some("review"));
    // The record is on disk, so the run rail lands on its row rather than
    // leaving the cursor where it was.
    assert_eq!(app.wf.run_index, Some(0));
    assert_eq!(app.wf.overlay.as_deref(), Some("run-1"));
}

#[test]
fn opening_a_run_with_no_record_yet_still_overlays_it() {
    let (_home, mut app) = app_with(&[diamond("review")]);

    app.open_workflow_run("review", "run-live");

    assert_eq!(app.selected_workflow().map(|w| w.id.as_str()), Some("review"));
    // No row to select — the record appears when the run settles — but the
    // overlay is what the node preview reads streamed output by, so it is set
    // regardless. Refusing the jump would leave the operator on the Agents rail
    // watching nothing.
    assert_eq!(app.wf.run_index, None);
    assert_eq!(app.wf.overlay.as_deref(), Some("run-live"));
}

#[test]
fn opening_a_run_of_an_unknown_workflow_says_so_rather_than_jumping() {
    let (_home, mut app) = app_with(&[diamond("review")]);

    app.open_workflow_run("gone", "run-1");

    // The tab still changes — that part is unconditional — but nothing is
    // selected or overlaid, and the operator is told why.
    assert_eq!(
        app.status(),
        "Workflow 'gone' is not in this catalogue",
        "the status names the missing workflow"
    );
    assert_eq!(app.wf.overlay, None);
}
