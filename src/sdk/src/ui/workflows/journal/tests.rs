//! Tests for the journal and proposal view.
//!
//! Mostly about what stays *visible*. A superseded note and a proposal that
//! will not apply are both things the model deliberately keeps on screen, and
//! both are the kind of thing a later refactor quietly filters out.

use super::*;
use crate::workflows::run::{Diagnosis, HiddenError};
use crate::workflows::{NoteKind, ProposalVerification};
use serde_json::json;

fn note(id: &str, kind: NoteKind, source: NoteSource, text: &str) -> WorkflowNote {
    WorkflowNote {
        id: id.into(),
        workflow_id: "sweep".into(),
        kind,
        text: text.into(),
        recorded_at: 1,
        source,
        run_ids: Vec::new(),
        superseded_by: None,
        pinned: false,
    }
}

fn proposal(
    status: ProposalStatus,
    verification: Option<ProposalVerification>,
) -> WorkflowProposal {
    WorkflowProposal {
        id: "p1".into(),
        workflow_id: "sweep".into(),
        created_at: 1,
        rationale: "the build step times out on a cold cache".into(),
        ops: json!([]),
        evidence_runs: vec!["run-1".into(), "run-2".into()],
        note_ids: Vec::new(),
        base_fingerprint: "abc".into(),
        verification,
        status,
        decided_at: None,
        decision_reason: None,
    }
}

fn passing() -> ProposalVerification {
    ProposalVerification {
        ok: true,
        verified_at: 2,
        messages: Vec::new(),
        diagnosis: None,
    }
}

fn failing() -> ProposalVerification {
    ProposalVerification {
        ok: false,
        verified_at: 2,
        messages: vec!["node 'build' does not exist".into()],
        diagnosis: None,
    }
}

#[test]
fn a_note_row_says_what_kind_it_is_and_who_wrote_it() {
    let rows = note_rows(&[note(
        "n1",
        NoteKind::Constraint,
        NoteSource::Operator,
        "never deploy before tests",
    )]);

    assert_eq!(rows[0].label, "never deploy before tests");
    assert!(rows[0].detail.contains("constraint"));
    // An operator's own words must be distinguishable from a model's guess.
    assert!(rows[0].detail.contains("you"));
    assert!(!rows[0].degraded);
}

#[test]
fn an_agent_note_names_the_model_when_it_is_known() {
    let named = note_rows(&[note(
        "n1",
        NoteKind::Hypothesis,
        NoteSource::Agent {
            model: Some("claude-opus-5".into()),
        },
        "the cache is cold on the first run of the day",
    )]);
    assert!(named[0].detail.contains("claude-opus-5"));

    let anonymous = note_rows(&[note(
        "n2",
        NoteKind::Hypothesis,
        NoteSource::Agent { model: None },
        "same",
    )]);
    assert!(anonymous[0].detail.contains("agent"));
}

#[test]
fn a_host_written_note_reads_as_observed_rather_than_reasoned() {
    let rows = note_rows(&[note(
        "n1",
        NoteKind::Observation,
        NoteSource::System,
        "Run run-9 failed.",
    )]);

    assert!(rows[0].detail.contains("observed"));
}

#[test]
fn a_superseded_note_is_dimmed_rather_than_dropped() {
    let mut superseded = note("n1", NoteKind::Hypothesis, NoteSource::System, "was wrong");
    superseded.superseded_by = Some("n2".into());

    let rows = note_rows(&[superseded]);

    assert_eq!(rows.len(), 1, "history stays on screen");
    assert!(rows[0].degraded);
}

#[test]
fn only_a_verified_pending_proposal_is_actionable() {
    assert!(actionable(&[proposal(ProposalStatus::Pending, None)]).is_none());
    assert!(actionable(&[proposal(ProposalStatus::Pending, Some(failing()))]).is_none());
    assert!(actionable(&[proposal(ProposalStatus::Accepted, Some(passing()))]).is_none());
    assert!(actionable(&[proposal(ProposalStatus::Pending, Some(passing()))]).is_some());
}

#[test]
fn display_prefers_the_same_applicable_proposal_the_keys_will_decide() {
    let mut failed = proposal(ProposalStatus::Pending, Some(failing()));
    failed.id = "newer-failed".into();
    let mut ready = proposal(ProposalStatus::Pending, Some(passing()));
    ready.id = "older-ready".into();
    let proposals = vec![failed, ready];

    assert_eq!(displayed(&proposals).unwrap().id, "older-ready");
    assert_eq!(
        displayed(&proposals).unwrap().id,
        actionable(&proposals).unwrap().id
    );
}

#[test]
fn a_failed_proposal_remains_visible_when_nothing_is_actionable() {
    let proposals = vec![proposal(ProposalStatus::Pending, Some(failing()))];

    assert!(actionable(&proposals).is_none());
    assert_eq!(displayed(&proposals).unwrap().id, proposals[0].id);
}

#[test]
fn a_proposal_row_distinguishes_ready_from_unapplicable() {
    let ready = proposal_rows(&[proposal(ProposalStatus::Pending, Some(passing()))]);
    assert_eq!(ready[0].detail, "ready");
    assert!(!ready[0].degraded);

    let broken = proposal_rows(&[proposal(ProposalStatus::Pending, Some(failing()))]);
    assert_eq!(broken[0].detail, "will not apply");
    assert!(broken[0].degraded);

    let unchecked = proposal_rows(&[proposal(ProposalStatus::Pending, None)]);
    assert_eq!(unchecked[0].detail, "unchecked");
}

#[test]
fn a_decided_proposal_says_which_way_it_went() {
    for (status, expected) in [
        (ProposalStatus::Accepted, "accepted"),
        (ProposalStatus::Rejected, "rejected"),
        (ProposalStatus::Stale, "stale"),
    ] {
        let rows = proposal_rows(&[proposal(status, Some(passing()))]);
        assert_eq!(rows[0].detail, expected);
    }
}

#[test]
fn the_detail_leads_with_why_and_names_the_evidence() {
    let rows = proposal_detail(&proposal(ProposalStatus::Pending, Some(passing())));

    let why = rows
        .iter()
        .find(|row| row.label == "why")
        .expect("a reason");
    assert!(why.value.contains("cold cache"));
    let evidence = rows
        .iter()
        .find(|row| row.label == "from runs")
        .expect("the evidence");
    assert_eq!(evidence.value, "run-1, run-2");
}

#[test]
fn a_failed_check_spells_out_what_went_wrong() {
    let rows = proposal_detail(&proposal(ProposalStatus::Pending, Some(failing())));

    // "will not apply" alone leaves an operator with nothing to do about it.
    assert!(rows
        .iter()
        .any(|row| row.value.contains("node 'build' does not exist")));
}

#[test]
fn a_failed_check_renders_every_blocking_diagnosis_category() {
    let mut diagnosis = Diagnosis::default();
    diagnosis.empty_prompts.push("summarize".into());
    diagnosis.hidden_errors.push(HiddenError {
        node_id: "notify".into(),
        message: Some("connection refused".into()),
    });
    let verification = ProposalVerification {
        ok: false,
        verified_at: 2,
        messages: Vec::new(),
        diagnosis: Some(diagnosis),
    };

    let rows = proposal_detail(&proposal(ProposalStatus::Pending, Some(verification)));

    assert!(rows
        .iter()
        .any(|row| row.value.contains("summarize") && row.value.contains("empty prompt")));
    assert!(rows
        .iter()
        .any(|row| row.value.contains("notify") && row.value.contains("connection refused")));
}

#[test]
fn an_unchecked_proposal_says_so_rather_than_looking_fine() {
    let rows = proposal_detail(&proposal(ProposalStatus::Pending, None));

    let checked = rows
        .iter()
        .find(|row| row.label == "checked")
        .expect("a check line");
    assert_eq!(checked.value, "not yet");
}
