//! Checking a proposal without applying it.
//!
//! Three gates in order, each answering a different question: do these ops
//! apply to this graph at all, is the result a graph the engine and this host's
//! semantic gates accept, and does simulating it surface any new problem — a
//! binding that resolves to null, an error a policy would swallow.
//!
//! Note the third gate's limit: it proves the patched graph is *sound*, not
//! that it is an improvement. A patch that changes nothing useful simulates
//! perfectly. Judging whether a change is worth making is the operator's part,
//! which is why the rationale is what they are shown first.
//!
//! A proposal that fails any of them is still *persisted* with the reason. That
//! is deliberate: a failed check is evidence the next pass reads, and silently
//! dropping a bad proposal would let an agent re-derive the same broken edit
//! every time the same run history came back around.

use std::sync::Arc;

use crate::workflows::authoring::preview_workflow_ops;
use crate::workflows::{
    require, ProposalVerification, StoreWorkflowResolver, WorkflowError, WorkflowProposal,
    WorkflowStore,
};

/// Check `proposal` against the store's current graph.
///
/// Never fails: every outcome is a verification, because "this could not be
/// checked" is itself the answer an operator and the next pass both need.
pub async fn verify(
    store: &Arc<dyn WorkflowStore>,
    proposal: &WorkflowProposal,
) -> ProposalVerification {
    let verified_at = crate::clock::now_millis() as u64;

    // The ops are stored as raw JSON, so parsing is itself a check: a proposal
    // naming an op this engine version does not have fails here rather than
    // half-applying.
    let ops = match crate::workflows::ops::parse_ops(&proposal.ops) {
        Ok(ops) => ops,
        Err(err) => {
            return ProposalVerification {
                ok: false,
                verified_at,
                messages: messages_of(err),
                diagnosis: None,
            }
        }
    };

    // Applying and validating in one: `preview_workflow_ops` runs the ops,
    // then the engine's validation, then this host's semantic gates — the same
    // path a real edit takes, minus the write.
    let candidate = match preview_workflow_ops(store, &proposal.workflow_id, &ops) {
        Ok(graph) => graph,
        Err(err) => {
            return ProposalVerification {
                ok: false,
                verified_at,
                messages: messages_of(err),
                diagnosis: None,
            }
        }
    };

    let current = match require(store.as_ref(), &proposal.workflow_id) {
        Ok(record) => record,
        Err(err) => {
            return ProposalVerification {
                ok: false,
                verified_at,
                messages: messages_of(err),
                diagnosis: None,
            }
        }
    };
    let resolver = Arc::new(StoreWorkflowResolver::new(store.clone()));
    let input = serde_json::json!({});
    let baseline =
        crate::workflows::run::dry_run_graph(&current.graph, resolver.clone(), input.clone()).await;
    match crate::workflows::run::dry_run_graph(&candidate, resolver, input).await {
        Ok(result) => ProposalVerification {
            // Empty input is not representative for every workflow. Accept a
            // candidate that retains only findings already present in the
            // unmodified graph under the identical sample; verification is
            // about whether the proposal introduced a new problem.
            ok: result.diagnosis.is_clean()
                || baseline
                    .as_ref()
                    .is_ok_and(|base| has_no_new_blockers(&base.diagnosis, &result.diagnosis)),
            verified_at,
            // A clean diagnosis has nothing to say; an unclean one is already
            // spelled out in the structured findings below, so the messages
            // stay for the failures that never reached a simulation.
            messages: Vec::new(),
            diagnosis: Some(result.diagnosis),
        },
        Err(err) => ProposalVerification {
            ok: false,
            verified_at,
            messages: messages_of(err),
            diagnosis: None,
        },
    }
}

/// Whether every blocking candidate finding already existed in the baseline.
fn has_no_new_blockers(
    baseline: &crate::workflows::run::diagnose::Diagnosis,
    candidate: &crate::workflows::run::diagnose::Diagnosis,
) -> bool {
    candidate
        .null_bindings
        .iter()
        .filter(|binding| !binding.unverifiable)
        .all(|binding| baseline.null_bindings.contains(binding))
        && candidate
            .empty_prompts
            .iter()
            .all(|node| baseline.empty_prompts.contains(node))
        && candidate
            .hidden_errors
            .iter()
            .all(|error| baseline.hidden_errors.contains(error))
}

/// Flatten an error into the messages an operator reads.
///
/// [`WorkflowError::Invalid`] carries every failure rather than the first, and
/// keeping them apart is the difference between "your patch is wrong" and a
/// list of the four things to fix.
fn messages_of(err: WorkflowError) -> Vec<String> {
    match err {
        WorkflowError::Invalid { messages, .. } => messages,
        other => vec![other.to_string()],
    }
}
