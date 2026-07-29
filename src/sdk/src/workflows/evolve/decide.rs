//! Accepting or rejecting a proposal.
//!
//! [`accept`] is the only function in this feature that changes a saved
//! workflow. Everything else observes, records, or suggests. That is the whole
//! shape of the autonomy decision: an agent can reason about a workflow all it
//! likes, and a person decides whether the graph moves.
//!
//! Both outcomes write a note, for the same reason. A fix that landed is
//! something the next pass should know it already tried; a rejection is what
//! stops it proposing the same idea again the next time the same evidence comes
//! back around. Without the second, the loop terminates but never converges.

use std::sync::Arc;

use crate::workflows::authoring::apply_workflow_ops_if_unchanged;
use crate::workflows::store::require_proposal;
use crate::workflows::{
    fingerprint, mint_note_id, ops, require, NoteKind, NoteSource, ProposalStatus, WorkflowError,
    WorkflowNote, WorkflowProposal, WorkflowRecord, WorkflowStore,
};

/// Apply a pending proposal to the saved graph.
///
/// Routes through [`apply_workflow_ops`], so this is not a special path: the
/// semantic gates run, and the store snapshots the superseded graph as a
/// revision. An accepted proposal is undoable by the same key as any other
/// edit.
///
/// # Errors
///
/// Refuses a proposal that is not pending, one that failed verification, and —
/// most importantly — one whose base graph has changed since it was written.
/// Ops are positional edits against a specific graph; applying them to a
/// different one is not a merge but an arbitrary rewrite.
pub fn accept(
    store: &Arc<dyn WorkflowStore>,
    proposal_id: &str,
) -> Result<(WorkflowRecord, WorkflowProposal), WorkflowError> {
    // Claimed for the whole read-modify-write. Two accepts of one proposal —
    // a double key press, each spawned on its own blocking task — could
    // otherwise both read it as pending, both pass the fingerprint check, and
    // both apply the ops.
    let _claim = DecisionGuard::claim(&format!("proposal:{proposal_id}")).ok_or_else(|| {
        WorkflowError::Engine(format!("proposal '{proposal_id}' is already being applied"))
    })?;
    let mut proposal = require_proposal(store.as_ref(), proposal_id)?;
    // Different proposals for one workflow are also positional edits against
    // the same base graph. Hold this claim from status/fingerprint validation
    // through persistence so neither can validate against a graph the other
    // changes before applying its ops.
    let _workflow_claim = DecisionGuard::claim(&format!("workflow:{}", proposal.workflow_id))
        .ok_or_else(|| {
            WorkflowError::Engine(format!(
                "another proposal for workflow '{}' is already being applied",
                proposal.workflow_id
            ))
        })?;
    if !proposal.is_pending() {
        return Err(WorkflowError::Engine(format!(
            "proposal '{proposal_id}' is already {:?} and cannot be applied again",
            proposal.status
        )));
    }
    if !proposal.is_applicable() {
        return Err(WorkflowError::Engine(format!(
            "proposal '{proposal_id}' has not passed verification, so it cannot be applied; \
             re-verify it or reject it"
        )));
    }

    let current = require(store.as_ref(), &proposal.workflow_id)?;
    if fingerprint(&current.graph) != proposal.base_fingerprint {
        // Recorded rather than merely refused: an operator who comes back to
        // this proposal needs to see why it stopped being offered, and the next
        // pass should not keep re-verifying something that can never apply.
        proposal.status = ProposalStatus::Stale;
        proposal.decided_at = Some(crate::clock::now_millis() as u64);
        proposal.decision_reason = Some(
            "the workflow changed after this proposal was written, so its patches no longer \
             describe the graph they would be applied to"
                .to_string(),
        );
        store.save_proposal(&proposal)?;
        return Err(WorkflowError::Engine(format!(
            "proposal '{proposal_id}' was written against an earlier version of \
             '{}' and is now stale",
            proposal.workflow_id
        )));
    }

    let ops = ops::parse_ops(&proposal.ops)?;
    let Some(record) = apply_workflow_ops_if_unchanged(
        store,
        &proposal.workflow_id,
        &ops,
        &proposal.base_fingerprint,
    )?
    else {
        proposal.status = ProposalStatus::Stale;
        proposal.decided_at = Some(crate::clock::now_millis() as u64);
        proposal.decision_reason =
            Some("the workflow changed while this proposal was being applied".to_string());
        store.save_proposal(&proposal)?;
        return Err(WorkflowError::Engine(format!(
            "proposal '{proposal_id}' became stale while it was being applied"
        )));
    };

    let decided_at = crate::clock::now_millis() as u64;
    proposal.status = ProposalStatus::Accepted;
    proposal.decided_at = Some(decided_at);
    store.save_proposal(&proposal)?;

    // Best effort: the graph has already changed, and failing here would leave
    // the caller unable to tell whether the edit landed.
    note(
        store,
        &proposal,
        NoteKind::Fix,
        decided_at,
        format!("Applied: {}", proposal.rationale.trim()),
    );

    Ok((record, proposal))
}

/// Turn a proposal down, with a reason.
///
/// The reason is not decoration. It becomes a [`NoteKind::Rejection`] note, and
/// that note is what a later pass reads to know this idea has already been
/// considered and declined.
pub fn reject(
    store: &Arc<dyn WorkflowStore>,
    proposal_id: &str,
    reason: &str,
) -> Result<WorkflowProposal, WorkflowError> {
    let _claim = DecisionGuard::claim(&format!("proposal:{proposal_id}")).ok_or_else(|| {
        WorkflowError::Engine(format!("proposal '{proposal_id}' is already being decided"))
    })?;
    let mut proposal = require_proposal(store.as_ref(), proposal_id)?;
    if !proposal.is_pending() {
        return Err(WorkflowError::Engine(format!(
            "proposal '{proposal_id}' is already {:?}",
            proposal.status
        )));
    }

    let decided_at = crate::clock::now_millis() as u64;
    proposal.status = ProposalStatus::Rejected;
    proposal.decided_at = Some(decided_at);
    let reason = reason.trim();
    if !reason.is_empty() {
        proposal.decision_reason = Some(reason.to_string());
    }
    store.save_proposal(&proposal)?;

    let text = if reason.is_empty() {
        format!("Rejected: {}", proposal.rationale.trim())
    } else {
        format!("Rejected: {}. {reason}", proposal.rationale.trim())
    };
    note(store, &proposal, NoteKind::Rejection, decided_at, text);

    Ok(proposal)
}

/// Record a decision as a note, logging rather than failing if it cannot be.
///
/// A note that could not be written must not make a decision that already
/// landed look like it failed. It is pinned and attributed to the operator
/// because a decision is theirs, not the agent's — which also keeps it safe
/// from the journal's cap.
fn note(
    store: &Arc<dyn WorkflowStore>,
    proposal: &WorkflowProposal,
    kind: NoteKind,
    recorded_at: u64,
    text: String,
) {
    let note = WorkflowNote {
        id: mint_note_id(recorded_at),
        workflow_id: proposal.workflow_id.clone(),
        kind,
        text,
        recorded_at,
        source: NoteSource::Operator,
        run_ids: proposal.evidence_runs.clone(),
        superseded_by: None,
        pinned: true,
    };
    if let Err(err) = store.append_note(&note) {
        tracing::warn!(
            workflow = %proposal.workflow_id,
            proposal = %proposal.id,
            "could not record the decision as a note: {err}"
        );
    }
}

/// Serializes decisions on one proposal or workflow within this process.
///
/// A guard rather than a flag on the record: the window that matters is between
/// reading the proposal and writing the graph, and only something that releases
/// on drop closes it against a task that panics halfway.
struct DecisionGuard(String);

impl DecisionGuard {
    /// The one set every guard shares.
    ///
    /// A single accessor rather than a `static` in each method: two `OnceLock`s
    /// spelled the same way are two different sets, and `Drop` would then clear
    /// one the claim never touched — a lock that silently never unlocks.
    fn in_flight() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
        static IN_FLIGHT: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
            std::sync::OnceLock::new();
        IN_FLIGHT.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
    }

    fn claim(key: &str) -> Option<Self> {
        let mut held = Self::in_flight()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !held.insert(key.to_string()) {
            return None;
        }
        Some(Self(key.to_string()))
    }
}

impl Drop for DecisionGuard {
    fn drop(&mut self) {
        Self::in_flight()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&self.0);
    }
}
