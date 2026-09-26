//! Tests for accepting and declining workflow proposals from the TUI.

use medulla::workflows::{ProposalStatus, ProposalVerification, WorkflowProposal};
use serde_json::json;

use super::super::types::PromptKind;
use super::tests::{app_with, diamond};

/// A pending proposal whose verification failed.
fn failed_proposal(workflow_id: &str) -> WorkflowProposal {
    WorkflowProposal {
        id: "failed-proposal".into(),
        workflow_id: workflow_id.into(),
        created_at: 1,
        rationale: "replace a step".into(),
        ops: json!([]),
        evidence_runs: Vec::new(),
        note_ids: Vec::new(),
        base_fingerprint: "fingerprint".into(),
        verification: Some(ProposalVerification {
            ok: false,
            verified_at: 1,
            messages: vec!["the patch does not apply".into()],
            diagnosis: None,
        }),
        status: ProposalStatus::Pending,
        decided_at: None,
        decision_reason: None,
    }
}

#[test]
fn a_failed_pending_proposal_can_be_declined() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    app.workflow_store()
        .save_proposal(&failed_proposal("sweep"))
        .expect("save proposal");
    app.reload_workflows();

    assert!(app.reject_selected_proposal().is_none());
    assert!(matches!(
        app.prompt.as_ref().map(|prompt| &prompt.kind),
        Some(PromptKind::RejectProposal {
            workflow,
            proposal_id,
        }) if workflow == "sweep" && proposal_id == "failed-proposal"
    ));
}
