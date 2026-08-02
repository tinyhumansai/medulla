//! Tests for the workflow row builders.

use super::*;
use crate::workflows::{RunRecord, RunStatus, RunStep, WorkflowSummary};

fn summary(id: &str, enabled: bool) -> WorkflowSummary {
    WorkflowSummary {
        id: id.to_string(),
        name: "Nightly sweep".into(),
        description: "sweeps the repo".into(),
        enabled,
        node_count: 3,
        trigger_kind: Some("manual".into()),
        inputs: Vec::new(),
    }
}

fn run(id: &str, status: RunStatus) -> RunRecord {
    RunRecord {
        id: id.to_string(),
        workflow_id: "sweep".into(),
        status,
        started_at: 1,
        finished_at: Some(2),
        steps: vec![RunStep {
            node_id: "a".into(),
            status: "success".into(),
            duration_ms: 5,
            input: None,
            output: None,
            diagnostics: Vec::new(),
        }],
        pending_approvals: Vec::new(),
        error: None,
        summary: None,
        diagnosis: None,
    }
}

#[test]
fn a_workflow_row_says_how_it_starts_how_big_it_is_and_what_it_does() {
    let rows = workflow_rows(&[summary("sweep", true)]);

    assert_eq!(rows[0].key, "workflow:sweep");
    assert_eq!(rows[0].label, "Nightly sweep");
    assert_eq!(rows[0].detail, "manual · 3 steps · sweeps the repo");
    assert!(!rows[0].degraded);
}

#[test]
fn a_disabled_workflow_says_so_first_and_renders_dim() {
    let rows = workflow_rows(&[summary("sweep", false)]);

    assert!(
        rows[0].detail.starts_with("disabled"),
        "the reason it will not run belongs first: {}",
        rows[0].detail
    );
    assert!(rows[0].degraded);
}

#[test]
fn a_single_step_workflow_is_not_described_in_the_plural() {
    let one = WorkflowSummary {
        node_count: 1,
        ..summary("tiny", true)
    };

    assert!(workflow_rows(&[one])[0].detail.contains("1 step ·"));
}

#[test]
fn a_run_awaiting_approval_names_what_it_is_waiting_on() {
    let mut parked = run("r1", RunStatus::PendingApproval);
    parked.pending_approvals = vec!["review".into(), "deploy".into()];

    let rows = run_rows(&[parked]);

    assert!(
        rows[0].detail.contains("awaiting review, deploy"),
        "{}",
        rows[0].detail
    );
    assert!(
        !rows[0].degraded,
        "a run an operator can still act on is not history"
    );
}

#[test]
fn a_failed_run_carries_its_error_and_renders_dim() {
    let mut failed = run("r2", RunStatus::Failed);
    failed.error = Some("the harness exploded".into());

    let rows = run_rows(&[failed]);

    assert!(rows[0].detail.contains("failed"));
    assert!(rows[0].detail.contains("the harness exploded"));
    assert!(rows[0].degraded);
}

#[test]
fn a_running_run_is_not_dimmed_because_it_is_the_thing_being_watched() {
    assert!(!run_rows(&[run("r3", RunStatus::Running)])[0].degraded);
}

#[test]
fn every_status_has_a_word_and_a_colour() {
    for status in [
        RunStatus::Running,
        RunStatus::PendingApproval,
        RunStatus::Succeeded,
        RunStatus::Failed,
        RunStatus::Cancelled,
        RunStatus::Interrupted,
    ] {
        assert!(!status_label(status).is_empty(), "{status:?}");
        assert!(!status_color(status).is_empty(), "{status:?}");
    }
    assert_eq!(status_color(RunStatus::Succeeded), "green");
    assert_eq!(status_color(RunStatus::Running), "yellow");
    assert_eq!(status_color(RunStatus::PendingApproval), "yellow");
    assert_eq!(status_color(RunStatus::Failed), "red");
    assert_eq!(status_color(RunStatus::Cancelled), "red");
    assert_eq!(status_color(RunStatus::Interrupted), "red");
}
