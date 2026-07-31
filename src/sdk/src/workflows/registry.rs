//! Resolving a workflow id to a graph for the engine.
//!
//! `sub_workflow` nodes name another workflow by id, and the engine asks the
//! host to produce that graph through [`tinyflows::caps::WorkflowResolver`].
//! This is the whole of that seam: a lookup, with absence and failure reported
//! as capability errors. Cycle and depth limits belong to the engine
//! ([`tinyflows::engine::MAX_SUB_WORKFLOW_DEPTH`]), not here — a resolver that
//! tried to police recursion would only disagree with it.
//!
//! **A resolved child keeps its own `defaults`.** The engine shares one
//! [`tinyflows::caps::Capabilities`] bundle across a run and every
//! `sub_workflow` it expands ([`crate::flow_engine::HostServices`] is built
//! once, in [`crate::workflows::run::run_workflow`]), so there is no seam left
//! by the time a child's `agent` nodes execute for the run-wide settings to
//! notice which workflow they came from. [`WorkflowResolver::resolve`] itself
//! returns a bare [`WorkflowGraph`] with nowhere to carry a `defaults` block
//! either. So this is the one place that still knows both the child record and
//! its graph: before handing the graph back, [`apply_defaults`] bakes the
//! child's own `defaults` directly into every `agent` node that names no
//! harness or model of its own — folding the workflow-level layer into the
//! node-level one the dispatch path already reads, rather than leaving it to
//! silently fall through to the parent run's settings.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tinyflows::caps::WorkflowResolver;
use tinyflows::error::{EngineError, Result};
use tinyflows::model::{NodeKind, WorkflowGraph};

use crate::flow_engine::{HarnessChoice, HarnessPreference};
use crate::workflows::store::WorkflowStore;
use crate::workflows::WorkflowDefaults;

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
            Ok(Some(mut record)) => {
                apply_defaults(&mut record.graph, &record.defaults);
                Ok(record.graph)
            }
            Ok(None) => Err(EngineError::Capability(format!(
                "sub_workflow: no saved workflow found for workflow_id '{workflow_id}'"
            ))),
            Err(err) => Err(EngineError::Capability(format!(
                "sub_workflow: failed to load workflow_id '{workflow_id}': {err}"
            ))),
        }
    }
}

/// Bake `defaults` into every `agent` node of `graph` that names no harness or
/// model of its own.
///
/// The dispatch path resolves a node's choice from exactly two layers — the
/// node's own config, then the run's standing settings (see
/// [`crate::flow_engine::caps::agent::HarnessAgentRunner::choice_for`]) — with
/// no notion of "which workflow is this node's own". Writing the resolved
/// node-plus-defaults pair back into the node's `config` makes it look, from
/// that call site's perspective, as if the node had stated the preference
/// itself: the run's settings (host, or a parent workflow's own `defaults`)
/// still apply as the fallback when neither the node nor this workflow's
/// `defaults` name anything, exactly as they would for a top-level run.
///
/// A malformed `harness` value already failed authoring-time validation for a
/// saved record, so a node this cannot parse is left untouched rather than
/// failing resolution — the engine will still raise it as a capability error
/// once the node itself executes.
fn apply_defaults(graph: &mut WorkflowGraph, defaults: &WorkflowDefaults) {
    if defaults.is_empty() {
        return;
    }
    let Ok(workflow_layer) = defaults.preference() else {
        // Cannot happen for a record that passed authoring validation, but a
        // resolver has no business panicking over a corrupted document.
        return;
    };
    for node in graph.nodes.iter_mut().filter(|n| n.kind == NodeKind::Agent) {
        let Ok(node_layer) = HarnessPreference::from_config(&node.config) else {
            continue;
        };
        let choice = HarnessChoice::resolve(&[node_layer, workflow_layer.clone()]);
        merge_choice_into_config(&mut node.config, &choice);
    }
}

/// Write a resolved [`HarnessChoice`] back into a node's `config` under the
/// canonical `harness` / `model` keys.
///
/// `choice` already gives the node's own preference priority over the
/// workflow's (see [`HarnessChoice::resolve`]), so overwriting unconditionally
/// is safe: when the node named its own harness or model, the resolved value
/// *is* that value, just normalized onto the `harness` key even if the node
/// originally spelled it `provider`.
fn merge_choice_into_config(config: &mut Value, choice: &HarnessChoice) {
    let harness = choice
        .provider
        .map(|provider| provider.as_str().to_string())
        .or_else(|| choice.custom_harness.clone());
    let Some(object) = config.as_object_mut() else {
        return;
    };
    if let Some(harness) = harness {
        object.insert("harness".to_string(), Value::String(harness));
    }
    if let Some(model) = &choice.model {
        object.insert("model".to_string(), Value::String(model.clone()));
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
