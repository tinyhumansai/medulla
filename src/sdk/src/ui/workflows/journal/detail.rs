//! Renders the full verification and decision detail for one proposal.

use crate::ui::workflows::inspect::DetailRow;
use crate::workflows::WorkflowProposal;

use super::rows::status_label;

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
