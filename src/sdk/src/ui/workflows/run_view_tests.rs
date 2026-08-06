//! What a run's overview shows, and what it refuses to leave out.

use serde_json::json;

use crate::workflows::{new_run_record, RunOrigin, RunRecord, RunStatus, RunStep};

use super::*;

/// A settled run of `workflow`, with no inputs and one successful step.
fn settled() -> RunRecord {
    let mut record = new_run_record("run-abc-1234abcd", "sweep", 60_000);
    record.status = RunStatus::Succeeded;
    record.finished_at = Some(60_000 + 95_000);
    record.steps = vec![RunStep {
        node_id: "one".into(),
        status: "success".into(),
        duration_ms: 12,
        input: None,
        output: None,
        diagnostics: Vec::new(),
    }];
    record
}

/// Find the value of the first row with `label`.
fn row(rows: &[DetailRow], label: &str) -> Option<String> {
    rows.iter()
        .find(|row| row.label == label)
        .map(|row| row.value.clone())
}

#[test]
fn declared_inputs_get_a_row_each() {
    let record = settled().with_inputs(
        &json!({ "repo": "acme/api", "pr": 41, "dry": true })
            .as_object()
            .cloned()
            .expect("an object"),
        &json!({}),
    );
    let rows = run_overview(&record);
    assert_eq!(row(&rows, "input repo").as_deref(), Some("acme/api"));
    assert_eq!(row(&rows, "input pr").as_deref(), Some("41"));
    assert_eq!(row(&rows, "input dry").as_deref(), Some("true"));
    // A string input is shown as its text, not as a quoted JSON literal.
    assert!(!rows.iter().any(|row| row.value.contains('"')));
}

#[test]
fn a_workflow_with_no_inputs_says_so_rather_than_showing_nothing() {
    let rows = run_overview(&settled());
    assert_eq!(row(&rows, "inputs").as_deref(), Some("none"));
}

#[test]
fn a_non_empty_trigger_is_shown_and_an_empty_one_is_not() {
    let empty = settled().with_inputs(&Default::default(), &json!({}));
    assert!(row(&run_overview(&empty), "trigger").is_none());

    let carried = settled().with_inputs(&Default::default(), &json!({ "event": "push" }));
    assert_eq!(
        row(&run_overview(&carried), "trigger").as_deref(),
        Some(r#"{"event":"push"}"#)
    );
}

#[test]
fn an_oversized_input_is_cut_rather_than_pushing_every_other_row_off() {
    let long = "x".repeat(5_000);
    let record = settled().with_inputs(
        &json!({ "note": long })
            .as_object()
            .cloned()
            .expect("object"),
        &json!({}),
    );
    let value = row(&run_overview(&record), "input note").expect("the input is shown");
    assert!(value.chars().count() <= INPUT_VALUE_CHARS, "{value:?}");
    assert!(value.ends_with('…'));
}

#[test]
fn a_session_origin_names_the_session_it_came_from() {
    let record = settled().with_origin(Some(RunOrigin::session("pty-0000-feedface")));
    let value = row(&run_overview(&record), "started by").expect("an origin row");
    assert!(value.contains("feedface"), "{value}");
}

#[test]
fn an_unknown_origin_kind_is_shown_verbatim_rather_than_as_unknown() {
    let record = settled().with_origin(Some(RunOrigin::of_kind("webhook")));
    assert_eq!(
        row(&run_overview(&record), "started by").as_deref(),
        Some("webhook")
    );
}

#[test]
fn a_settled_run_states_that_nothing_failed() {
    assert_eq!(
        row(&run_overview(&settled()), "progress").as_deref(),
        Some("1 step · none failed")
    );
}

#[test]
fn a_failed_step_is_counted() {
    let mut record = settled();
    record.status = RunStatus::Failed;
    record.steps.push(RunStep {
        node_id: "two".into(),
        status: "error".into(),
        duration_ms: 3,
        input: None,
        output: None,
        diagnostics: Vec::new(),
    });
    assert_eq!(
        row(&run_overview(&record), "progress").as_deref(),
        Some("2 steps · 1 failed")
    );
}

#[test]
fn a_running_run_says_so_rather_than_reporting_a_duration() {
    let mut record = settled();
    record.status = RunStatus::Running;
    record.finished_at = None;
    let value = row(&run_overview(&record), "timing").expect("a timing row");
    assert!(value.ends_with("still running"), "{value}");
}

#[test]
fn a_settled_run_reports_how_long_it_took() {
    let value = row(&run_overview(&settled()), "timing").expect("a timing row");
    assert!(value.contains("1m 35s"), "{value}");
}

#[test]
fn durations_read_as_a_person_would_say_them() {
    assert_eq!(human_duration(400), "400ms");
    assert_eq!(human_duration(9_000), "9s");
    assert_eq!(human_duration(95_000), "1m 35s");
    assert_eq!(human_duration(3_800_000), "1h 3m");
}

#[test]
fn a_bounded_input_row_reads_as_the_text_it_was_given() {
    let mut record = settled();
    record.inputs = serde_json::json!({
        "instruction": {
            "_medullaTruncated": true,
            "originalBytes": 9_001,
            "preview": "rebuild the index",
        }
    })
    .as_object()
    .cloned()
    .expect("an object");
    assert_eq!(
        row(&run_overview(&record), "input instruction").as_deref(),
        Some("rebuild the index")
    );
}

#[test]
fn a_plain_string_input_is_not_quoted() {
    assert_eq!(value_text(&serde_json::json!("main")), "main");
    assert_eq!(value_text(&serde_json::json!(3)), "3");
}
