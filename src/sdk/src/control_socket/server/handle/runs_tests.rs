use serde_json::json;

use crate::control_socket::grants::Grant;
use crate::control_socket::runs::{HarnessRunRegistry, HarnessRunStatus};
use crate::control_socket::types::ErrorKind;

use super::{run_report, MAX_FIELD};

/// A grant for `session-1`, which is the session every report lands under.
fn grant() -> Grant {
    Grant::new("session-1", 0, 2)
}

#[test]
fn a_report_is_filed_under_the_grants_session_not_a_named_one() {
    let runs = HarnessRunRegistry::new();
    let params = json!({
        "runId": "run-1",
        "workflowId": "review",
        "status": "running",
        "detail": "reading src/",
        "node": "review",
        // Ignored: attribution comes from the grant, never from the caller.
        "session": "somebody-else",
    });
    run_report(&runs, &grant(), &params).expect("a well-formed report is accepted");

    assert!(runs.for_session("somebody-else").is_empty());
    let recorded = runs.for_session("session-1");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].run_id, "run-1");
    assert_eq!(recorded[0].status, HarnessRunStatus::Running);
    assert_eq!(recorded[0].frames[0].node.as_deref(), Some("review"));
}

#[test]
fn an_oversized_identifier_is_refused_rather_than_retained() {
    for field in ["runId", "workflowId", "node"] {
        let runs = HarnessRunRegistry::new();
        let mut params = json!({
            "runId": "run-1",
            "workflowId": "review",
            "status": "running",
            "detail": "x",
        });
        params[field] = json!("a".repeat(MAX_FIELD + 1));

        let failure =
            run_report(&runs, &grant(), &params).expect_err("an oversized {field} is refused");
        assert_eq!(failure.kind, ErrorKind::BadRequest, "for {field}");
        assert!(failure.message.contains(field), "for {field}");
        // Refused means nothing was retained, not merely that the caller was
        // told off after the fact.
        assert!(runs.for_session("session-1").is_empty(), "for {field}");
    }
}

#[test]
fn a_field_exactly_at_the_limit_is_still_accepted() {
    let runs = HarnessRunRegistry::new();
    let id = "a".repeat(MAX_FIELD);
    run_report(
        &runs,
        &grant(),
        &json!({ "runId": id, "workflowId": "review", "status": "running" }),
    )
    .expect("the limit is inclusive");
    assert_eq!(runs.for_session("session-1").len(), 1);
}

#[test]
fn a_detail_line_is_flattened_and_clipped_rather_than_refused() {
    let runs = HarnessRunRegistry::new();
    let noisy = format!("first\u{1b}[2Jline\n\nsecond {}", "x".repeat(4_000));
    run_report(
        &runs,
        &grant(),
        &json!({
            "runId": "run-1",
            "workflowId": "review",
            "status": "running",
            "detail": noisy,
        }),
    )
    .expect("a long detail line is a verbose harness, not a broken one");

    let recorded = runs.for_session("session-1");
    let detail = recorded[0].detail.as_deref().expect("a detail was recorded");
    assert!(!detail.contains('\u{1b}'), "control characters survived");
    assert!(!detail.contains('\n'));
    assert!(detail.len() < noisy.len());
}

#[test]
fn a_report_naming_no_run_is_refused() {
    let runs = HarnessRunRegistry::new();
    let failure = run_report(&runs, &grant(), &json!({ "status": "running" }))
        .expect_err("a report with no run id is refused");
    assert_eq!(failure.kind, ErrorKind::BadRequest);
    assert!(runs.for_session("session-1").is_empty());
}
