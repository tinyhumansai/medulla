//! What leaves the rail when it finishes, and what is pinned to it anyway.

use medulla::control_socket::{HarnessRunStatus, RunReport};

use super::tests::{hosting_app, stub_session};
use super::{run_rows_under, RailRow, SessionRailRow};
use crate::ui::harness_pane::HarnessFocus;
use crate::worker::pty::PtyState;

/// A pty row that has exited with `code`, carrying `grant` as its MCP grant.
fn exited(id: &str, code: i32, grant: Option<&str>) -> crate::worker::pty::SessionRow {
    let mut row = stub_session(id);
    row.state = PtyState::Exited { code: Some(code) };
    row.mcp_grant_session = grant.map(str::to_string);
    row
}

/// A report of `status` for `run` under `grant`.
fn report(status: HarnessRunStatus, run: &str) -> RunReport {
    RunReport {
        run_id: run.to_string(),
        workflow_id: "sweep".to_string(),
        status,
        detail: None,
        node: None,
    }
}

/// One session row wrapping `local`, as the rail hands it to `run_rows_under`.
fn rail_row(local: crate::worker::pty::SessionRow) -> SessionRailRow {
    SessionRailRow {
        agent_id: None,
        lane_index: None,
        task: None,
        local: Some(local),
        last: true,
    }
}

#[test]
fn a_failed_session_stays_visible_for_the_operator() {
    let app = hosting_app();

    assert!(
        app.keeps_finished_session(&exited("w_1", 1, None)),
        "the lifecycle cue must survive the next frame sweep"
    );
}

#[test]
fn the_attached_session_stays_listed_after_it_exits() {
    // The operator is reading that screen — often *because* it exited. Sweeping
    // it would take the exit code away from the person looking at it.
    let mut app = hosting_app();
    app.harness_focus = HarnessFocus::Attached("w_1".to_string());

    assert!(app.keeps_finished_session(&exited("w_1", 1, None)));
    // A clean exit is kept only while it is the screen being read.
    assert!(!app.keeps_finished_session(&exited("w_2", 0, None)));
}

#[test]
fn a_finished_session_whose_run_is_still_going_stays_listed() {
    // A detached run outlives its parent harness by design, and its rows are
    // drawn under the session that started it.
    let app = hosting_app();
    app.harness_runs
        .report("grant-1", report(HarnessRunStatus::Running, "run-1"));

    assert!(app.keeps_finished_session(&exited("w_1", 0, Some("grant-1"))));
}

#[test]
fn a_finished_session_whose_runs_have_all_settled_leaves() {
    let app = hosting_app();
    app.harness_runs
        .report("grant-1", report(HarnessRunStatus::Succeeded, "run-1"));
    app.harness_runs
        .report("grant-1", report(HarnessRunStatus::Failed, "run-2"));

    assert!(
        !app.keeps_finished_session(&exited("w_1", 0, Some("grant-1"))),
        "settled runs draw no rows, so they pin nothing"
    );
}

#[test]
fn a_cleanly_exited_session_without_runs_leaves() {
    let app = hosting_app();

    assert!(!app.keeps_finished_session(&exited("w_1", 0, None)));
}

#[test]
fn only_the_runs_still_executing_get_rows() {
    let app = hosting_app();
    for (status, run) in [
        (HarnessRunStatus::Succeeded, "run-done"),
        (HarnessRunStatus::Failed, "run-failed"),
        (HarnessRunStatus::AwaitingApproval, "run-gated"),
        (HarnessRunStatus::Running, "run-live"),
    ] {
        app.harness_runs.report("grant-1", report(status, run));
    }

    let ids: Vec<String> = run_rows_under(
        &rail_row(exited("w_1", 0, Some("grant-1"))),
        &app.harness_runs,
    )
    .into_iter()
    .filter_map(|row| match row {
        RailRow::WorkflowRun(run) => Some(run.run.run_id),
        _ => None,
    })
    .collect();

    assert_eq!(
        ids,
        vec!["run-gated".to_string(), "run-live".to_string()],
        "a run parked on an approval gate is still in flight; a settled one is \
         history the Workflows page owns"
    );
}

#[test]
fn the_last_glyph_follows_the_runs_that_survive_the_filter() {
    // `last` is what draws the group's closing branch. Computed over the
    // filtered rows, or a settled tail would leave the group looking unclosed.
    let app = hosting_app();
    app.harness_runs
        .report("grant-1", report(HarnessRunStatus::Running, "run-live"));
    app.harness_runs
        .report("grant-1", report(HarnessRunStatus::Succeeded, "run-done"));

    let rows = run_rows_under(
        &rail_row(exited("w_1", 0, Some("grant-1"))),
        &app.harness_runs,
    );

    assert_eq!(rows.len(), 1);
    assert!(matches!(&rows[0], RailRow::WorkflowRun(run) if run.last));
}
