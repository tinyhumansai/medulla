//! One evolution pass, start to finish.
//!
//! The ordering in [`EvolveSession::evolve`] is the design. The system note is
//! written *before* anything is dispatched, so a pass still teaches the
//! workflow something when the harness is missing, the turn times out, or the
//! agent replies with nothing but prose. Everything after that point is best
//! effort layered on a durable floor.

use std::sync::Arc;

use tokio::sync::mpsc;

use super::context::record_failure_note;
use super::registry::EvolveGuard;
use super::types::{EvolveConfig, EvolveOutcome, EvolveTrigger};
use crate::flow_engine::caps::dispatch::HarnessDispatch;
use crate::hub::TaskRequest;
use crate::mcp::ToolMode;
use crate::workflows::copilot::{CopilotRequest, FailedRun, Mode};
use crate::workflows::{
    current_notes, require, NoteKind, NoteSource, RunRecord, RunStatus, WorkflowError,
    WorkflowNote, WorkflowStore,
};

/// Everything a pass needs to reach a harness.
///
/// Deliberately the same shape as [`crate::workflows::copilot::CopilotSession`]
/// — a pass is a copilot turn with a different brief and a smaller tool set,
/// not a different kind of thing.
pub struct EvolveSession {
    /// The store the workflow, its runs, and its journal come from.
    pub store: Arc<dyn WorkflowStore>,
    /// Where the review is run.
    pub dispatch: Arc<dyn HarnessDispatch>,
    /// The bridge address of the worker that runs it.
    pub worker_address: String,
    /// Optional harness hint.
    pub provider: Option<crate::tinyplace::HarnessProvider>,
    /// Optional model hint.
    pub model: Option<String>,
    /// The continuity group this pane's turns share.
    pub conversation: String,
    /// How much history reaches the brief.
    pub config: EvolveConfig,
}

impl EvolveSession {
    /// Review a workflow against its own history.
    ///
    /// # Errors
    ///
    /// Fails when the workflow is unknown or when the dispatch itself fails. A
    /// turn that proposed nothing is not a failure, and neither is one whose
    /// proposal did not verify — both are outcomes with something recorded.
    ///
    /// Note what is *not* an error: a pass already running for this workflow
    /// comes back as [`EvolveOutcome::skipped`]. A workflow failing in a burst
    /// is the normal case, and the second failure has nothing to say that the
    /// first pass will not already find.
    pub async fn evolve(
        &self,
        workflow_id: &str,
        trigger: EvolveTrigger,
        status: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<EvolveOutcome, WorkflowError> {
        if !self.config.enabled {
            return Err(WorkflowError::Engine(
                "workflow evolution is disabled on this host (workflows.evolve.enabled = false)"
                    .to_string(),
            ));
        }
        let record = require(self.store.as_ref(), workflow_id)?;

        // Claimed before the note is written, so a burst of failures produces
        // one pass rather than ten harnesses reaching the same conclusion.
        let store_scope = self.store.proposal_decision_scope();
        let Some(_guard) = EvolveGuard::claim(&store_scope, workflow_id) else {
            return Ok(EvolveOutcome {
                skipped: true,
                ..Default::default()
            });
        };

        // The durable floor. Before any dispatch, deliberately: this is the
        // part that still happens when there is no harness to ask.
        let mut notes = Vec::new();
        if let Some(run) = self.triggering_run(workflow_id, &trigger)? {
            match record_failure_note(&self.store, &run) {
                // `None` means `run_workflow` already wrote it, which is the
                // normal case on the auto-triggered path.
                Ok(Some(note)) => notes.push(note),
                Ok(None) => {}
                // Logged rather than propagated: a pass that could not write
                // its note has still learned nothing, and failing here would
                // lose the turn as well.
                Err(err) => {
                    tracing::warn!(workflow = %workflow_id, "could not record the run: {err}")
                }
            }
        }

        let known = current_notes(self.store.as_ref(), workflow_id)?;
        let briefed_notes = select_for_brief(known, self.config.max_notes);
        let runs = truncate(self.store.list_runs(workflow_id)?, self.config.max_runs);
        let notes_before: Vec<String> = self
            .store
            .list_notes(workflow_id)?
            .into_iter()
            .map(|note| note.id)
            .collect();
        let proposals_before: Vec<String> = self
            .store
            .list_proposals(workflow_id)?
            .into_iter()
            .map(|proposal| proposal.id)
            .collect();

        let prompt = CopilotRequest {
            mode: Mode::Evolve,
            instruction: instruction_for(&trigger),
            record: Some(&record),
            run: self.failed_run(workflow_id, &trigger)?,
            notes: &briefed_notes,
            runs: &runs,
        }
        .render();

        let task_id = format!("evolve-{}", uuid::Uuid::new_v4());
        let request = TaskRequest {
            task_id: task_id.clone(),
            abort_id: task_id,
            cycle_id: None,
            instruction: prompt,
            worker_address: self.worker_address.clone(),
            provider: self.provider,
            custom_harness: None,
            model: self.model.clone(),
            // The autonomy boundary, carried to the harness rather than
            // assumed. Without this the review turn is served the full
            // authoring surface and the "it cannot edit" claim is only prose.
            tool_mode: Some(format!("{}:{workflow_id}", ToolMode::Propose.as_wire())),
            // Never a workflow: this is a review turn. Setting it would run the
            // graph the pass is reviewing, which for a failure-triggered pass
            // would be the run that triggered it, again.
            workflow: None,
            conversation: Some(self.conversation.clone()),
            fleet_depth: 0,
        };
        let outcome = self.dispatch.dispatch_with_status(request, status).await?;

        // Read back off the store rather than from the reply. The tools wrote,
        // and the store is the only thing that knows what they wrote — the same
        // principle `copilot::diff` already applies to graph edits.
        let already_kept: Vec<String> = notes.iter().map(|note| note.id.clone()).collect();
        notes.extend(self.store.list_notes(workflow_id)?.into_iter().filter(
            |note: &WorkflowNote| {
                !notes_before.contains(&note.id) && !already_kept.contains(&note.id)
            },
        ));

        let mut proposals = Vec::new();
        for mut proposal in self.store.list_proposals(workflow_id)? {
            if proposals_before.contains(&proposal.id) {
                continue;
            }
            // `workflow_propose` verifies as it stores, but a pass that reached
            // the store another way, or one whose graph moved mid-turn, would
            // otherwise offer something unchecked.
            if proposal.verification.is_none() {
                crate::workflows::ops::verify_proposal(&self.store, &proposal.id).await?;
                proposal = crate::workflows::require_proposal(self.store.as_ref(), &proposal.id)?;
            }
            proposals.push(proposal);
        }

        Ok(EvolveOutcome {
            reply: outcome.reply.trim().to_string(),
            notes,
            proposals,
            skipped: false,
        })
    }

    /// The run this pass is about, when it is about one.
    ///
    /// A trigger naming a run the store has lost is not fatal: the pass still
    /// has the journal and the rest of the history to work from.
    fn triggering_run(
        &self,
        workflow_id: &str,
        trigger: &EvolveTrigger,
    ) -> Result<Option<RunRecord>, WorkflowError> {
        let Some(run_id) = trigger.run_id() else {
            return Ok(None);
        };
        let Some(run) = self.store.get_run(run_id)? else {
            return Ok(None);
        };
        if run.workflow_id != workflow_id {
            return Err(WorkflowError::Malformed(format!(
                "run '{run_id}' belongs to workflow '{}', not '{workflow_id}'",
                run.workflow_id
            )));
        }
        if run.status != RunStatus::Failed {
            return Err(WorkflowError::Malformed(format!(
                "run '{run_id}' is {:?}, not a failed run",
                run.status
            )));
        }
        Ok(Some(run))
    }

    /// The failing run, in the shape the brief wants it.
    fn failed_run(
        &self,
        workflow_id: &str,
        trigger: &EvolveTrigger,
    ) -> Result<Option<FailedRun>, WorkflowError> {
        Ok(self
            .triggering_run(workflow_id, trigger)?
            .map(|run| FailedRun {
                id: run.id.clone(),
                error: run.error.clone(),
                failing_nodes: super::context::failing_nodes(&run),
            }))
    }
}

/// Keep the newest `limit` of a newest-first listing.
fn truncate<T>(mut items: Vec<T>, limit: usize) -> Vec<T> {
    items.truncate(limit);
    items
}

/// Choose which notes reach the brief, by weight rather than by age alone.
///
/// Recency alone would quietly break the thing this feature is named for.
/// Every failed run writes an observation, so after enough failures a
/// `Rejection` — the note that stops a review re-proposing an idea already
/// turned down — falls off the end of the budget, and the loop starts
/// re-deriving it. Operator constraints age out the same way, which would let a
/// model's own guess outweigh what a person actually said.
///
/// So the durable claims are taken first, newest-first within each tier, and
/// observations fill whatever budget is left.
pub(crate) fn select_for_brief(notes: Vec<WorkflowNote>, limit: usize) -> Vec<WorkflowNote> {
    if notes.len() <= limit {
        return notes;
    }
    let mut chosen: Vec<WorkflowNote> = Vec::with_capacity(limit);
    // Two passes over one newest-first list, so ordering inside a tier is still
    // chronological and the caller sees one coherent sequence.
    for keep in [true, false] {
        for note in &notes {
            if chosen.len() == limit {
                break;
            }
            if is_durable(note) == keep {
                chosen.push(note.clone());
            }
        }
    }
    chosen.sort_by(|a, b| b.id.cmp(&a.id));
    chosen
}

/// Whether a note is a standing claim rather than a record of one moment.
fn is_durable(note: &WorkflowNote) -> bool {
    note.pinned
        || matches!(note.source, NoteSource::Operator)
        || matches!(
            note.kind,
            NoteKind::Rejection | NoteKind::Constraint | NoteKind::Fix
        )
}

/// What to tell the agent it was asked, given why the pass started.
///
/// The brief's directive already says what an evolve turn does; this is the
/// occasion for it. A failure pass and a manual review want different opening
/// questions even though the work is the same.
fn instruction_for(trigger: &EvolveTrigger) -> &'static str {
    match trigger {
        EvolveTrigger::Failure(_) => {
            "A run of this workflow just failed. Work out what it teaches you about the \
             workflow — not only about this one run — and record it. Propose a change only \
             if the evidence supports one; a single failure that looks like a transient \
             fault is worth noting and nothing more."
        }
        EvolveTrigger::Manual => {
            "Review this workflow against everything recorded about it and say how it \
             could be better. Look for patterns across runs rather than reacting to the \
             most recent one."
        }
    }
}
