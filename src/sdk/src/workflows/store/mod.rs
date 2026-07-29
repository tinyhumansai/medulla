//! Where workflows and their run records live.
//!
//! The engine has exactly one seam for "where does a graph come from" —
//! [`tinyflows::caps::WorkflowResolver`] — and it only covers resolving a
//! `sub_workflow` node's id. Everything else a host needs (listing, saving,
//! deleting, recording runs) has no contract upstream, so this module defines
//! one: [`WorkflowStore`].
//!
//! The trait exists so the backing store is a decision, not a fact of the
//! codebase. [`FileWorkflowStore`] — JSON documents under `.medulla/workflows`,
//! matching how agent templates are already kept — is the only implementation
//! today, but a remote catalog or a database is a new impl rather than a
//! refactor.

mod file;

#[cfg(test)]
mod concurrency_tests;
#[cfg(test)]
mod tests;

pub use file::{
    mint_note_id, mint_proposal_id, new_run_record, parse_workflow, validate_graph, workflow_dirs,
    FileWorkflowStore, LoadReport, MAX_NOTES, MAX_REVISIONS,
};

use crate::workflows::types::{
    RunId, RunRecord, WorkflowId, WorkflowNote, WorkflowProposal, WorkflowRecord, WorkflowRevision,
    WorkflowSummary,
};
use crate::workflows::WorkflowError;

/// Persistence for workflow definitions and their run history.
///
/// Implementations are shared across threads and may be called from async
/// contexts, so they must be `Send + Sync`; the methods are synchronous because
/// every backing store in view is either local files or an in-process database,
/// and a blocking read there is cheaper than the machinery to avoid it. Callers
/// on an async runtime should wrap these in `spawn_blocking`, as the TUI already
/// does for its task repository.
pub trait WorkflowStore: Send + Sync {
    /// Every known workflow, in a stable display order.
    fn list(&self) -> Result<Vec<WorkflowSummary>, WorkflowError>;

    /// One workflow by id, or `None` when the store has no such record.
    fn get(&self, id: &str) -> Result<Option<WorkflowRecord>, WorkflowError>;

    /// Write `record`, replacing any existing workflow with the same id.
    ///
    /// Implementations validate before writing: a store never persists a graph
    /// the engine would refuse to compile, so a listing can be trusted to be
    /// runnable.
    fn save(&self, record: &WorkflowRecord) -> Result<(), WorkflowError>;

    /// Save only when the current graph still has `expected_fingerprint`.
    ///
    /// File-backed stores override this atomically. The default preserves the
    /// contract for lightweight test stores that do not expose transactions.
    fn save_if_fingerprint(
        &self,
        record: &WorkflowRecord,
        expected_fingerprint: &str,
    ) -> Result<bool, WorkflowError> {
        let Some(current) = self.get(&record.id)? else {
            return Ok(false);
        };
        if crate::workflows::fingerprint(&current.graph) != expected_fingerprint {
            return Ok(false);
        }
        self.save(record)?;
        Ok(true)
    }

    /// Remove a workflow. Removing one that does not exist is an error, so a
    /// caller cannot mistake a typo for a successful delete.
    fn delete(&self, id: &str) -> Result<(), WorkflowError>;

    /// Write a run record, replacing any earlier state for the same run id.
    fn record_run(&self, run: &RunRecord) -> Result<(), WorkflowError>;

    /// One run by id.
    fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, WorkflowError>;

    /// Every recorded run for a workflow, newest first.
    fn list_runs(&self, workflow_id: &str) -> Result<Vec<RunRecord>, WorkflowError>;

    /// Every superseded copy of a workflow, newest first.
    ///
    /// A workflow that has never been written over has no revisions, which is
    /// an empty listing rather than an error.
    fn list_revisions(&self, workflow_id: &str) -> Result<Vec<WorkflowRevision>, WorkflowError>;

    /// One superseded copy, by id, scoped to the workflow it belongs to.
    ///
    /// Scoped rather than global so a rollback cannot reach another workflow's
    /// history — that would be a way to write a graph the operator never had.
    fn revision(
        &self,
        workflow_id: &str,
        revision_id: &str,
    ) -> Result<Option<WorkflowRevision>, WorkflowError>;

    /// Every note recorded about a workflow, newest first, including notes a
    /// later one superseded.
    ///
    /// Defaulted so a store that keeps no journal — a read-only catalogue, a
    /// test stand-in — is not obliged to invent one. The asymmetry with
    /// [`WorkflowStore::append_note`] is deliberate: reporting "nothing
    /// learned" is honest, whereas silently discarding something the host
    /// claims to have learned is not.
    fn list_notes(&self, workflow_id: &str) -> Result<Vec<WorkflowNote>, WorkflowError> {
        let _ = workflow_id;
        Ok(Vec::new())
    }

    /// Record a note.
    fn append_note(&self, note: &WorkflowNote) -> Result<(), WorkflowError> {
        let _ = note;
        Err(WorkflowError::Engine(
            "this workflow store does not keep notes".to_string(),
        ))
    }

    /// Mark a note as replaced by a later one, returning whether it changed.
    ///
    /// The superseded note stays listed; it simply stops being briefed.
    fn supersede_note(
        &self,
        workflow_id: &str,
        note_id: &str,
        by: &str,
    ) -> Result<bool, WorkflowError> {
        let _ = (workflow_id, note_id, by);
        Err(WorkflowError::Engine(
            "this workflow store does not keep notes".to_string(),
        ))
    }

    /// Write a proposal, replacing any earlier state for the same id.
    ///
    /// Every transition goes through here — verified, accepted, rejected, made
    /// stale — so a proposal's stored form is its current state rather than a
    /// log to replay.
    fn save_proposal(&self, proposal: &WorkflowProposal) -> Result<(), WorkflowError> {
        let _ = proposal;
        Err(WorkflowError::Engine(
            "this workflow store does not keep proposals".to_string(),
        ))
    }

    /// One proposal by id.
    fn get_proposal(&self, id: &str) -> Result<Option<WorkflowProposal>, WorkflowError> {
        let _ = id;
        Ok(None)
    }

    /// Every proposal for a workflow, newest first, decided ones included.
    fn list_proposals(&self, workflow_id: &str) -> Result<Vec<WorkflowProposal>, WorkflowError> {
        let _ = workflow_id;
        Ok(Vec::new())
    }
}

/// Fetch a proposal by id, turning absence into an error.
pub fn require_proposal(
    store: &dyn WorkflowStore,
    id: &str,
) -> Result<WorkflowProposal, WorkflowError> {
    store
        .get_proposal(id)?
        .ok_or_else(|| WorkflowError::Malformed(format!("no proposal with id '{id}'")))
}

/// A workflow's current notes — what a brief should be built from.
///
/// Superseded notes are deliberately excluded: they are history worth showing
/// an operator, but asking a model to reason from a claim already known to be
/// wrong is worse than telling it nothing.
pub fn current_notes(
    store: &dyn WorkflowStore,
    workflow_id: &str,
) -> Result<Vec<WorkflowNote>, WorkflowError> {
    Ok(store
        .list_notes(workflow_id)?
        .into_iter()
        .filter(WorkflowNote::is_current)
        .collect())
}

/// Restore `workflow_id` to the state held by `revision_id`.
///
/// Goes through [`WorkflowStore::save`], so the graph being replaced is itself
/// snapshotted first: a rollback is undoable by the same key that performed it.
///
/// # Errors
///
/// Fails when the workflow or the revision is unknown, or when the restored
/// graph no longer validates — which can happen if a sub-workflow it referenced
/// has since been deleted.
pub fn rollback(
    store: &dyn WorkflowStore,
    workflow_id: &str,
    revision_id: &str,
) -> Result<WorkflowRecord, WorkflowError> {
    let revision = store.revision(workflow_id, revision_id)?.ok_or_else(|| {
        WorkflowError::Malformed(format!(
            "workflow '{workflow_id}' has no revision '{revision_id}'"
        ))
    })?;
    store.save(&revision.record)?;
    Ok(revision.record)
}

/// Restore `workflow_id` to the state before its most recent edit.
///
/// What the operator's undo key calls. Returns `None` when there is no history
/// to go back to, which the caller reports rather than treating as a failure —
/// a workflow that has never been edited is a normal thing to press undo on.
pub fn undo_last(
    store: &dyn WorkflowStore,
    workflow_id: &str,
) -> Result<Option<(WorkflowRevision, WorkflowRecord)>, WorkflowError> {
    let Some(newest) = store.list_revisions(workflow_id)?.into_iter().next() else {
        return Ok(None);
    };
    let restored = rollback(store, workflow_id, &newest.id)?;
    Ok(Some((newest, restored)))
}

/// Fetch a workflow by id, turning "no such workflow" into an error.
///
/// The common case at a command boundary, where absence is a failure to report
/// rather than a state to branch on.
pub fn require(store: &dyn WorkflowStore, id: &str) -> Result<WorkflowRecord, WorkflowError> {
    store
        .get(id)?
        .ok_or_else(|| WorkflowError::NotFound(WorkflowId::from(id)))
}

/// Fetch a run by id, turning absence into an error.
pub fn require_run(store: &dyn WorkflowStore, run_id: &str) -> Result<RunRecord, WorkflowError> {
    store
        .get_run(run_id)?
        .ok_or_else(|| WorkflowError::RunNotFound(RunId::from(run_id)))
}
