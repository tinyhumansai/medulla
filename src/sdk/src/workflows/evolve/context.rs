//! Gathering what a pass reasons from, and the one note it always writes.
//!
//! The system note here is the load-bearing part of the whole feature. It is
//! built from a run record alone — no model, no dispatch, no network — so a
//! host that has no harness installed, whose turn timed out, or whose agent
//! replied with nothing but prose still *learns something* from every failure.
//! Everything else in this module is best effort on top of that.

use std::sync::Arc;

use crate::workflows::{
    mint_note_id, NoteKind, NoteSource, RunRecord, RunStatus, WorkflowError, WorkflowNote,
    WorkflowStore,
};

/// The nodes a run recorded as having errored.
pub fn failing_nodes(run: &RunRecord) -> Vec<String> {
    run.steps
        .iter()
        .filter(|step| step.status == "error")
        .map(|step| step.node_id.clone())
        .collect()
}

/// Whether this run's failure has already been recorded.
///
/// Checked at the source rather than at each trigger, because there are three
/// of them: `run_workflow` writes the note synchronously, and both the TUI and
/// the daemon then start a review for the same run. Without this the primary
/// path records every failure twice — filling the journal's cap at double rate
/// and showing a model the same observation as if it were two pieces of
/// evidence.
fn already_recorded(store: &Arc<dyn WorkflowStore>, run: &RunRecord) -> bool {
    store
        .list_notes(&run.workflow_id)
        .unwrap_or_default()
        .iter()
        .any(|note| {
            note.source == NoteSource::System
                && note.kind == NoteKind::Observation
                && note.run_ids.iter().any(|id| id == &run.id)
        })
}

/// Write the observation a failed run always earns.
///
/// Attributed to [`NoteSource::System`] rather than to an agent, because no
/// model was involved: this is the host restating a run record. Keeping that
/// distinction is what lets a brief weight it differently from a claim
/// something reasoned its way to.
///
/// Returns `None` when this run's failure is already in the journal, which is
/// the normal case for the second caller on the primary path.
///
/// # Errors
///
/// Propagates a store failure so a caller that must know can. The triggers
/// deliberately log rather than propagate — see their call sites.
pub fn record_failure_note(
    store: &Arc<dyn WorkflowStore>,
    run: &RunRecord,
) -> Result<Option<WorkflowNote>, WorkflowError> {
    // Only a failure earns an observation. The TUI filters to failed runs
    // before it triggers, but `medulla workflow evolve --run-id` takes whatever
    // it is given, and recording "Run X ended as Succeeded" as evidence would
    // put noise in the journal and in every brief after it.
    if run.status != RunStatus::Failed || already_recorded(store, run) {
        return Ok(None);
    }
    let recorded_at = crate::clock::now_millis() as u64;
    let note = WorkflowNote {
        id: mint_note_id(recorded_at),
        workflow_id: run.workflow_id.clone(),
        kind: NoteKind::Observation,
        text: describe_failure(run),
        recorded_at,
        source: NoteSource::System,
        run_ids: vec![run.id.clone()],
        superseded_by: None,
        pinned: false,
    };
    store.append_note(&note)?;
    Ok(Some(note))
}

/// State a failure in one paragraph, without proposing a cause.
///
/// An observation makes no claim about *why*: a note that guesses becomes
/// evidence for the next guess, and two passes later the journal is arguing
/// with itself about something nobody observed.
fn describe_failure(run: &RunRecord) -> String {
    let mut text = format!("Run {} failed.", run.id);
    if let Some(summary) = run
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        text.push(' ');
        text.push_str(summary);
        if !summary.ends_with('.') {
            text.push('.');
        }
    }
    if let Some(error) = run
        .error
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        text.push_str(&format!(" Error: {error}"));
    }
    let failing = failing_nodes(run);
    if !failing.is_empty() {
        text.push_str(&format!(" Failing nodes: {}.", failing.join(", ")));
    }
    if let Some(diagnosis) = &run.diagnosis {
        if !diagnosis.null_bindings.is_empty() {
            let bindings: Vec<String> = diagnosis
                .null_bindings
                .iter()
                .map(|binding| format!("{} ({})", binding.location, binding.expression))
                .collect();
            text.push_str(&format!(" Resolved to null: {}.", bindings.join("; ")));
        }
        if !diagnosis.hidden_errors.is_empty() {
            let hidden: Vec<String> = diagnosis
                .hidden_errors
                .iter()
                .map(|error| match error.message.as_deref() {
                    Some(message) => format!("{}: {message}", error.node_id),
                    None => error.node_id.clone(),
                })
                .collect();
            text.push_str(&format!(
                " Errors an on_error policy swallowed: {}.",
                hidden.join("; ")
            ));
        }
    }
    text
}
