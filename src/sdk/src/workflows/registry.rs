//! Resolving a workflow id to a graph for the engine.
//!
//! `sub_workflow` nodes name another workflow by id, and the engine asks the
//! host to produce that graph through [`tinyflows::caps::WorkflowResolver`].
//! This is the whole of that seam: a lookup, with absence and failure reported
//! as capability errors. Cycle and depth limits belong to the engine
//! ([`tinyflows::engine::MAX_SUB_WORKFLOW_DEPTH`]), not here — a resolver that
//! tried to police recursion would only disagree with it.

use std::sync::Arc;

use async_trait::async_trait;
use tinyflows::caps::WorkflowResolver;
use tinyflows::error::{EngineError, Result};
use tinyflows::model::WorkflowGraph;

use crate::workflows::store::WorkflowStore;

/// A [`WorkflowResolver`] over any [`WorkflowStore`].
pub struct StoreWorkflowResolver {
    store: Arc<dyn WorkflowStore>,
}

impl StoreWorkflowResolver {
    /// Resolve sub-workflows out of `store`.
    pub fn new(store: Arc<dyn WorkflowStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl WorkflowResolver for StoreWorkflowResolver {
    async fn resolve(&self, workflow_id: &str) -> Result<WorkflowGraph> {
        match self.store.get(workflow_id) {
            // Disabling a workflow has to mean it does not run, including as
            // somebody else's child: the check in `run_workflow` guards only
            // the root graph.
            Ok(Some(record)) if !record.enabled => Err(EngineError::Capability(format!(
                "sub_workflow: workflow '{workflow_id}' is disabled"
            ))),
            Ok(Some(record)) => Ok(record.graph),
            Ok(None) => Err(EngineError::Capability(format!(
                "sub_workflow: no saved workflow found for workflow_id '{workflow_id}'"
            ))),
            Err(err) => Err(EngineError::Capability(format!(
                "sub_workflow: failed to load workflow_id '{workflow_id}': {err}"
            ))),
        }
    }
}
