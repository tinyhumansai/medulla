//! Editing a workflow as a series of patches.
//!
//! Workflows here are written by agents as often as by people, and an agent
//! editing a graph by rewriting the whole JSON document loses information every
//! time it misremembers a field. The engine's [`GraphOp`] patch language exists
//! for exactly this: small, named, checkable edits — add a node, merge-patch a
//! config, rewire an edge — that fail loudly rather than silently dropping what
//! they did not mention.
//!
//! Every edit here is apply → validate → save, in that order, and a graph that
//! fails validation is never written. An author's mistake costs them an error
//! message, not their saved workflow.

use std::sync::Arc;

use tinyflows::graph_ops::{apply_ops, GraphOp};
use tinyflows::model::WorkflowGraph;

use crate::workflows::store::{require, validate_graph};
use crate::workflows::{WorkflowError, WorkflowRecord, WorkflowStore};

/// Apply `ops` to the workflow `id` and save the result.
///
/// Returns the saved record. The workflow is left untouched if any op fails to
/// apply or the result fails validation — the ops are applied to a copy, and
/// only a graph that would compile is written back.
pub fn apply_workflow_ops(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    ops: &[GraphOp],
) -> Result<WorkflowRecord, WorkflowError> {
    // A copilot, CLI, and another MCP process may all edit the same file through
    // independent store instances. Rebase the patch when another writer wins
    // between read and save instead of reporting two successes while silently
    // discarding the earlier edit.
    const MAX_RETRIES: usize = 16;
    for _ in 0..MAX_RETRIES {
        let current = require(store.as_ref(), id)?;
        let expected = crate::workflows::record_fingerprint(&current);
        if let Some(record) = apply_workflow_ops_if_record_unchanged(store, id, ops, &expected)? {
            return Ok(record);
        }
    }
    Err(WorkflowError::Engine(format!(
        "workflow '{id}' kept changing while the edit was being saved; retry the edit"
    )))
}

/// Apply `ops` only if the graph still matches `expected_fingerprint`.
///
/// Returns `None` when the expected fingerprint is stale, including when the
/// graph changes between the initial read and persistence. Returns `Some` only
/// after applying the ops, validating the graph and host semantic gates, and
/// durably saving the result.
///
/// # Errors
///
/// Returns an error when the workflow is missing, an op cannot be applied, the
/// resulting graph fails validation or semantic checks, or persistence fails.
pub fn apply_workflow_ops_if_unchanged(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    ops: &[GraphOp],
    expected_fingerprint: &str,
) -> Result<Option<WorkflowRecord>, WorkflowError> {
    let mut record = require(store.as_ref(), id)?;
    if crate::workflows::fingerprint(&record.graph) != expected_fingerprint {
        return Ok(None);
    }
    record.graph = apply_ops(&record.graph, ops)
        .map_err(|err| WorkflowError::Engine(format!("workflow '{id}': {err}")))?;
    validate_graph(id, &record.graph)?;
    crate::workflows::gates::check(id, &record.graph)?;
    if !store.save_if_fingerprint(&record, expected_fingerprint)? {
        return Ok(None);
    }
    Ok(Some(record))
}

/// Apply graph ops only while the whole workflow record remains unchanged.
fn apply_workflow_ops_if_record_unchanged(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    ops: &[GraphOp],
    expected_fingerprint: &str,
) -> Result<Option<WorkflowRecord>, WorkflowError> {
    let mut record = require(store.as_ref(), id)?;
    if crate::workflows::record_fingerprint(&record) != expected_fingerprint {
        return Ok(None);
    }
    record.graph = apply_ops(&record.graph, ops)
        .map_err(|err| WorkflowError::Engine(format!("workflow '{id}': {err}")))?;
    validate_graph(id, &record.graph)?;
    crate::workflows::gates::check(id, &record.graph)?;
    if !store.save_if_record_fingerprint(&record, expected_fingerprint)? {
        return Ok(None);
    }
    Ok(Some(record))
}

/// Preview `ops` against the workflow `id` without saving.
///
/// The same checks as [`apply_workflow_ops`], minus the write. What an author
/// calls to see whether an edit is sound before committing to it.
pub fn preview_workflow_ops(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    ops: &[GraphOp],
) -> Result<WorkflowGraph, WorkflowError> {
    let record = require(store.as_ref(), id)?;
    let graph = apply_ops(&record.graph, ops)
        .map_err(|err| WorkflowError::Engine(format!("workflow '{id}': {err}")))?;
    validate_graph(id, &graph)?;
    crate::workflows::gates::check(id, &graph)?;
    Ok(graph)
}

/// Create a workflow from a whole graph document, replacing any existing one of
/// the same id.
///
/// Parses, then validates, then saves — the same order [`apply_workflow_ops`]
/// uses, and for the same reason. A document that parses is not necessarily a
/// graph the engine would compile, and a create path that skipped validation
/// would be the one way to get an unrunnable workflow into a store whose
/// listings are otherwise trustworthy.
pub fn create_workflow(
    store: &Arc<dyn WorkflowStore>,
    document: &str,
    id_fallback: &str,
) -> Result<WorkflowRecord, WorkflowError> {
    let record = crate::workflows::store::parse_workflow(document, id_fallback)
        .map_err(WorkflowError::Malformed)?;
    validate_graph(&record.id, &record.graph)?;
    crate::workflows::gates::check(&record.id, &record.graph)?;
    store.save(&record)?;
    Ok(record)
}

/// A graph an author handed in, resolved from one of the ways they can name it.
///
/// Two ways to say "the graph I mean" — a saved id, or an inline document — so
/// validate, preview, and dry-run all take the same argument whether the author
/// is editing something saved or checking something they have not saved yet.
pub enum GraphHandle<'a> {
    /// A workflow already in the store.
    Saved(&'a str),
    /// A graph document supplied inline.
    Inline(&'a str),
}

impl GraphHandle<'_> {
    /// Resolve to a record, without saving anything.
    pub fn resolve(&self, store: &Arc<dyn WorkflowStore>) -> Result<WorkflowRecord, WorkflowError> {
        match self {
            GraphHandle::Saved(id) => require(store.as_ref(), id),
            GraphHandle::Inline(document) => {
                crate::workflows::store::parse_workflow(document, "inline")
                    .map_err(WorkflowError::Malformed)
            }
        }
    }
}

/// Validate a graph the author has not necessarily saved.
///
/// Reports every failure, not the first, so one round-trip tells an author
/// everything wrong with what they wrote.
pub fn validate_handle(
    store: &Arc<dyn WorkflowStore>,
    handle: &GraphHandle<'_>,
) -> Result<WorkflowRecord, WorkflowError> {
    let record = handle.resolve(store)?;
    validate_graph(&record.id, &record.graph)?;
    crate::workflows::gates::check(&record.id, &record.graph)?;
    Ok(record)
}

#[cfg(test)]
#[path = "authoring_tests.rs"]
mod tests;
