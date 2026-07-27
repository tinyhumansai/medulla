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
mod tests;

pub use file::{
    new_run_record, parse_workflow, validate_graph, workflow_dirs, FileWorkflowStore, LoadReport,
};

use crate::workflows::types::{RunId, RunRecord, WorkflowId, WorkflowRecord, WorkflowSummary};
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

    /// Remove a workflow. Removing one that does not exist is an error, so a
    /// caller cannot mistake a typo for a successful delete.
    fn delete(&self, id: &str) -> Result<(), WorkflowError>;

    /// Write a run record, replacing any earlier state for the same run id.
    fn record_run(&self, run: &RunRecord) -> Result<(), WorkflowError>;

    /// One run by id.
    fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, WorkflowError>;

    /// Every recorded run for a workflow, newest first.
    fn list_runs(&self, workflow_id: &str) -> Result<Vec<RunRecord>, WorkflowError>;
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
