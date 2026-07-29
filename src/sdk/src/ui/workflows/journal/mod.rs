//! What a workflow has learned, and what it suggests changing, as rows.
//!
//! Notes and proposals want opposite treatments. Notes are many and passive —
//! an operator reads them to understand, not to act — so they render as a list
//! in the inspector. A proposal is singular and actionable, so it renders as a
//! detail block with its verification spelled out, next to the keys that decide
//! it.

use crate::ui::workflows::inspect::DetailRow;
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

/// What a proposal says about itself, for the inspector.
pub fn proposal_detail(proposal: &WorkflowProposal) -> Vec<DetailRow> {
    let mut rows = vec![
        DetailRow {
            label: "status".into(),
            value: status_label(proposal).to_string(),
        },
        DetailRow {
            label: "why".into(),
            value: proposal.rationale.trim().to_string(),
        },
    ];
    if !proposal.evidence_runs.is_empty() {
        rows.push(DetailRow {
            label: "from runs".into(),
            value: proposal.evidence_runs.join(", "),
        });
    }
    match &proposal.verification {
        None => rows.push(DetailRow {
            label: "checked".into(),
            value: "not yet".into(),
        }),
        Some(check) if check.ok => rows.push(DetailRow {
            label: "checked".into(),
            value: "applies cleanly and simulates without new problems".into(),
        }),
        Some(check) => {
            rows.push(DetailRow {
                label: "checked".into(),
                value: "will not apply".into(),
            });
            for message in &check.messages {
                rows.push(DetailRow {
                    label: String::new(),
                    value: message.clone(),
                });
            }
            for binding in check
                .diagnosis
                .iter()
                .flat_map(|diagnosis| diagnosis.null_bindings.iter())
            {
                rows.push(DetailRow {
                    label: String::new(),
                    value: format!(
                        "{} resolves to null ({})",
                        binding.location, binding.expression
                    ),
                });
            }
            for node_id in check
                .diagnosis
                .iter()
                .flat_map(|diagnosis| diagnosis.empty_prompts.iter())
            {
                rows.push(DetailRow {
                    label: String::new(),
                    value: format!("{node_id} would run with an empty prompt"),
                });
            }
            for error in check
                .diagnosis
                .iter()
                .flat_map(|diagnosis| diagnosis.hidden_errors.iter())
            {
                let detail = error
                    .message
                    .as_deref()
                    .map(|message| format!(": {message}"))
                    .unwrap_or_default();
                rows.push(DetailRow {
                    label: String::new(),
                    value: format!("{} hides an error{}", error.node_id, detail),
                });
            }
        }
    }
    if let Some(reason) = proposal.decision_reason.as_deref() {
        rows.push(DetailRow {
            label: "note".into(),
            value: reason.to_string(),
        });
    }
    rows
}

/// A proposal's state in one word an operator can scan.
fn status_label(proposal: &WorkflowProposal) -> &'static str {
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

#[cfg(test)]
mod tests;
