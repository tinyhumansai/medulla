//! Builds compact inspector rows and selects the proposal to display or act on.

use crate::ui::workflows::rows::WorkflowRow;
use crate::workflows::{NoteSource, ProposalStatus, WorkflowNote, WorkflowProposal};

/// One row per note, newest first, superseded ones marked rather than hidden.
///
/// Hidden would be wrong here even though a *brief* excludes them: an operator
/// reading history wants to see what was believed and when, and a note that
/// silently vanished is one they will go looking for.
pub fn note_rows(notes: &[WorkflowNote]) -> Vec<WorkflowRow> {
    notes
        .iter()
        .map(|note| WorkflowRow {
            key: note.id.clone(),
            label: note.text.trim().to_string(),
            detail: format!("{} · {}", kind_label(note), source_label(&note.source)),
            // Dim: still worth showing, no longer worth believing.
            degraded: note.superseded_by.is_some(),
        })
        .collect()
}

/// One row per proposal, newest first.
///
/// A proposal that failed verification is degraded rather than absent. It is
/// evidence — the next review reads it to avoid re-deriving the same broken
/// edit — and hiding it would make the same idea reappear indefinitely.
pub fn proposal_rows(proposals: &[WorkflowProposal]) -> Vec<WorkflowRow> {
    proposals
        .iter()
        .map(|proposal| WorkflowRow {
            key: proposal.id.clone(),
            label: proposal.rationale.trim().to_string(),
            detail: status_label(proposal).to_string(),
            degraded: !proposal.is_applicable(),
        })
        .collect()
}

/// The one proposal an operator could act on right now, if there is one.
///
/// At most one exists by construction: a review supersedes this workflow's
/// other undecided proposals as it stores its own. That is what lets the keys
/// act on "the proposal" without a cursor to pick one.
pub fn actionable(proposals: &[WorkflowProposal]) -> Option<&WorkflowProposal> {
    proposals.iter().find(|proposal| proposal.is_applicable())
}

/// The newest proposal still awaiting a decision, including failed checks.
pub fn pending(proposals: &[WorkflowProposal]) -> Option<&WorkflowProposal> {
    proposals
        .iter()
        .find(|proposal| proposal.status == ProposalStatus::Pending)
}

/// The proposal the inspector should display.
///
/// Prefer the proposal the decision keys will act on. A failed pending
/// proposal remains visible when there is no applicable one, because its
/// diagnostics are still useful evidence.
pub fn displayed(proposals: &[WorkflowProposal]) -> Option<&WorkflowProposal> {
    actionable(proposals).or_else(|| pending(proposals))
}

/// A proposal's state in one word an operator can scan.
pub(super) fn status_label(proposal: &WorkflowProposal) -> &'static str {
    match proposal.status {
        ProposalStatus::Accepted => "accepted",
        ProposalStatus::Rejected => "rejected",
        ProposalStatus::Stale => "stale",
        // Two different pending states, and conflating them is the one thing
        // that would mislead: one is waiting on a person, the other cannot be
        // applied at all.
        ProposalStatus::Pending if proposal.is_applicable() => "ready",
        ProposalStatus::Pending if proposal.verification.is_some() => "will not apply",
        ProposalStatus::Pending => "unchecked",
    }
}

/// A note's kind, lowercased for the detail column.
fn kind_label(note: &WorkflowNote) -> String {
    format!("{:?}", note.kind).to_lowercase()
}

/// Who wrote a note, in one word.
fn source_label(source: &NoteSource) -> String {
    match source {
        NoteSource::Operator => "you".to_string(),
        NoteSource::System => "observed".to_string(),
        NoteSource::Agent { model: Some(model) } => model.clone(),
        NoteSource::Agent { model: None } => "agent".to_string(),
    }
}
