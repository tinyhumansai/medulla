//! What a run decides and refuses before its first node executes.
//!
//! Everything here happens between "a caller asked for this workflow" and "the
//! engine is running it": resolving the settings the run will use, refusing a
//! graph that must not execute at all, clamping what the host will not allow,
//! and taking out the claim that makes the run cancellable and unique.
//!
//! Grouped together because they share one property that the run body does not:
//! each is a decision made against the *unstarted* run, and each can still
//! refuse it outright. Once [`run_workflow`](super::run_workflow) is past this
//! point it has a record to reconcile and a guard to release, so a failure there
//! means something quite different.

use std::sync::Arc;

use crate::flow_engine::CapabilitySettings;
use crate::workflows::WorkflowError;

use super::registry::{RunClaim, RunGuard};

/// Lowers any `loop` node's `max_iterations` to this host's ceiling.
///
/// A loop's cap is the author's decision, but here a pass through the body can
/// be a whole harness session, so a graph asking for a thousand of them is
/// asking for something the operator never agreed to.
///
/// **Clamped, never refused** — the same contract as `max_parallel_agents`. A
/// workflow that asks for more still runs; it just stops sooner. Refusing would
/// make a graph authored against a more generous host unrunnable here, and the
/// author usually cannot change this host's configuration.
///
/// Applied to the execution graph only. The stored document keeps the number
/// the author wrote, so raising the ceiling later restores the intent without
/// anyone having to re-edit the workflow.
///
/// Called on every resolved graph, not just the root:
/// [`StoreWorkflowResolver`](crate::workflows::StoreWorkflowResolver) applies
/// this to a `sub_workflow`'s child graph too, with the same ceiling, so a
/// nested loop cannot outrun the host limit just because it is one
/// `sub_workflow` hop away from the run that declared it.
pub(crate) fn clamp_loop_iterations(
    mut graph: tinyflows::model::WorkflowGraph,
    ceiling: u64,
) -> tinyflows::model::WorkflowGraph {
    for node in &mut graph.nodes {
        if node.kind != tinyflows::model::NodeKind::Loop {
            continue;
        }
        // An absent cap falls through to the engine's own default
        // (`tinyflows::nodes::control_flow::loop_node::DEFAULT_MAX_ITERATIONS`),
        // which can itself exceed a low host ceiling — so it is treated as
        // that declared value here rather than left for the engine to apply
        // unclamped.
        let declared = node
            .config
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .unwrap_or(tinyflows::nodes::control_flow::loop_node::DEFAULT_MAX_ITERATIONS);
        if declared <= ceiling {
            continue;
        }
        // A `loop` node whose config is not an object — `null`, or anything
        // else a hand-edited document may hold — still needs the ceiling. The
        // config is replaced with one carrying it rather than skipped: skipping
        // would leave the node running to the engine's own default while the
        // log line above claimed it had been clamped. Nothing is lost, because
        // a non-object config held no settings to begin with.
        match node.config.as_object_mut() {
            Some(config) => {
                config.insert("max_iterations".to_string(), ceiling.into());
            }
            None => {
                node.config = serde_json::json!({ "max_iterations": ceiling });
            }
        }
        tracing::info!(
            node = %node.id,
            declared,
            ceiling,
            "clamping loop max_iterations to the host ceiling"
        );
    }
    graph
}

/// The settings one run of `workflow` uses.
///
/// The host's settings with the workflow's own `defaults` block layered over
/// them, so an `agent` node that names no harness of its own inherits the
/// workflow's rather than the machine's. Returned unchanged — and without a
/// clone of the whole struct — when the workflow states no preference, which is
/// most workflows.
///
/// Applied here rather than inside the dispatch path because the run is where
/// the workflow record and the capability settings are both in hand, and because
/// a sub-workflow's nodes should then inherit *its* defaults, not its parent's.
///
/// # Errors
///
/// Returns [`WorkflowError::Invalid`] when the block names something that cannot
/// be a harness. A stored workflow is checked on the way in, so this only fires
/// for a record built in memory.
pub(super) fn settings_for(
    host: &Arc<CapabilitySettings>,
    workflow: &crate::workflows::WorkflowRecord,
) -> Result<Arc<CapabilitySettings>, WorkflowError> {
    if workflow.defaults.is_empty() {
        return Ok(host.clone());
    }
    let preference = workflow
        .defaults
        .preference()
        .map_err(|message| WorkflowError::Invalid {
            id: workflow.id.clone(),
            messages: vec![format!("`defaults`: {message}")],
        })?;
    let choice =
        crate::flow_engine::HarnessChoice::resolve(&[preference, host.harness_preference()]);
    let mut settings = CapabilitySettings::clone(host);
    settings.default_provider = choice.provider;
    settings.default_custom_harness = choice.custom_harness;
    settings.default_model = choice.model;
    Ok(Arc::new(settings))
}

/// Refuse to run `workflow` if any `agent` node's `harness`/`provider` is an
/// unresolved `=`-expression.
///
/// [`crate::workflows::gates::harness_failures`] is the same check
/// `authoring::apply_ops`/`create` run before a write lands — but a graph can
/// reach a run without ever passing through that write path (an operator hand-
/// editing the saved JSON file directly, or a future store implementation that
/// does not route through authoring), and by the time a node dispatches the
/// engine has already resolved its config: an expression is indistinguishable
/// from a name typed by hand. This is the last point before that happens where
/// the raw, unresolved graph is still in hand, so it is the last point this can
/// still be caught rather than silently letting upstream data or a model's own
/// output choose which binary and which credentials run the step.
///
/// # Errors
///
/// Returns [`WorkflowError::Invalid`] naming every offending node.
pub(super) fn refuse_unresolved_harness(
    workflow: &crate::workflows::WorkflowRecord,
) -> Result<(), WorkflowError> {
    let failures = crate::workflows::gates::harness_failures(&workflow.graph);
    if failures.is_empty() {
        return Ok(());
    }
    Err(WorkflowError::Invalid {
        id: workflow.id.clone(),
        messages: failures,
    })
}

/// The claim `run_id` will execute under: the one the caller handed down, or a
/// fresh one.
///
/// A caller that already handed its run id out claims earlier still and passes
/// the claim down (see [`RunContext::claim`](crate::workflows::RunContext::claim));
/// adopting it is what keeps that earlier claim from reading as "already
/// executing" here.
///
/// # Errors
///
/// Returns [`WorkflowError::Engine`] when `handed` is a claim for a *different*
/// id, and when there is no claim to adopt and `run_id` is already claimed.
///
/// A mismatch is refused rather than quietly re-claimed under the right id:
/// adopting it would leave the registry holding only that other id, so a cancel
/// for this run would find nothing to stop, and a second dispatch of this id
/// could start alongside this one — the two outcomes claiming exists to prevent.
/// That combination is a caller wiring bug, and it should say so.
pub(super) fn adopt_or_claim(
    run_id: &str,
    handed: Option<RunClaim>,
) -> Result<RunClaim, WorkflowError> {
    let claim = match handed {
        Some(claim) if claim.0.id() != run_id => {
            return Err(WorkflowError::Engine(format!(
                "run '{run_id}' was handed a claim for '{}'",
                claim.0.id()
            )));
        }
        Some(claim) => Some(claim),
        None => RunGuard::claim(run_id),
    };
    claim.ok_or_else(|| WorkflowError::Engine(format!("run '{run_id}' is already executing")))
}
