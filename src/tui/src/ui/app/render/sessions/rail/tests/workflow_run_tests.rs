//! Tests for Sessions rail workflow-run entries.

use super::super::super::super::color;
use super::super::workflow_run_elapsed;
use super::{app, lane, none_waiting, NOW};

/// A reported run, as the control plane hands one to the rail.
fn reported_run(
    status: medulla::control_socket::HarnessRunStatus,
) -> medulla::control_socket::HarnessRun {
    medulla::control_socket::HarnessRun {
        run_id: "run-1".into(),
        workflow_id: "review-and-fix".into(),
        status,
        started_at: 1,
        updated_at: 2,
        detail: Some("review · running the test suite".into()),
        frames: Vec::new(),
    }
}

fn row(
    status: medulla::control_socket::HarnessRunStatus,
    last: bool,
) -> crate::ui::app::rail::RailRow {
    crate::ui::app::rail::RailRow::WorkflowRun(crate::ui::app::rail::WorkflowRunRailRow {
        session_id: "w_1".into(),
        run: reported_run(status),
        last,
    })
}

#[test]
fn a_workflow_run_row_names_its_workflow_status_and_elapsed_time() {
    let text = app()
        .rail_row_line(
            &row(medulla::control_socket::HarnessRunStatus::Running, true),
            &[lane()],
            false,
            &none_waiting(),
            NOW,
        )
        .to_string();
    assert!(text.contains("review-and-fix"));
    assert!(text.contains("running"));
    assert!(text.contains("9s"));
    assert!(text.starts_with("   └"));
}

#[test]
fn a_workflow_run_row_carries_no_harness_output() {
    let text = app()
        .rail_row_line(
            &row(medulla::control_socket::HarnessRunStatus::Running, true),
            &[lane()],
            false,
            &none_waiting(),
            NOW,
        )
        .to_string();
    assert!(!text.contains("running the test suite"));
}

#[test]
fn a_settled_run_row_stops_ageing_at_its_last_report() {
    let mut run = reported_run(medulla::control_socket::HarnessRunStatus::Succeeded);
    run.started_at = 1_000;
    run.updated_at = 4_000;
    let row =
        crate::ui::app::rail::RailRow::WorkflowRun(crate::ui::app::rail::WorkflowRunRailRow {
            session_id: "w_1".into(),
            run,
            last: true,
        });
    assert!(app()
        .rail_row_line(&row, &[lane()], false, &none_waiting(), NOW + 600_000)
        .to_string()
        .contains("3s"));
}

#[test]
fn clock_clamps_an_active_run_that_starts_after_now_to_zero() {
    let mut active = reported_run(medulla::control_socket::HarnessRunStatus::Running);
    active.started_at = NOW + 1;
    assert_eq!(
        workflow_run_elapsed(&active, NOW),
        medulla::ui::workflows::human_duration(0)
    );
}

#[test]
fn clock_clamps_a_settled_run_reported_before_it_started_to_zero() {
    let mut settled = reported_run(medulla::control_socket::HarnessRunStatus::Succeeded);
    settled.started_at = NOW;
    settled.updated_at = NOW - 1;
    assert_eq!(
        workflow_run_elapsed(&settled, NOW + 1),
        medulla::ui::workflows::human_duration(0)
    );
}

#[test]
fn a_failed_run_row_is_coloured_by_its_status_rather_than_by_the_row() {
    let line = app().rail_row_line(
        &row(medulla::control_socket::HarnessRunStatus::Failed, false),
        &[lane()],
        false,
        &none_waiting(),
        NOW,
    );
    let status = line
        .spans
        .iter()
        .find(|span| span.content.contains("failed"))
        .expect("a status span");
    assert_eq!(status.style.fg, Some(color("red")));
}
