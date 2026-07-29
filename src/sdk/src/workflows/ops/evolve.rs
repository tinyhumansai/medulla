//! Journal and proposal operations, as JSON in and JSON out.
//!
//! The same bargain as the rest of this surface: the CLI, the MCP tools, and
//! the TUI call these rather than the store directly, so none of the three can
//! drift from the others.
//!
//! [`accept_proposal`] and [`reject_proposal`] are here and reachable from the
//! CLI and the TUI, but deliberately not exposed as MCP tools — see
//! `mcp::tools::evolve`.

use std::sync::Arc;

use serde_json::{json, Value};

use super::record_value;
use crate::workflows::evolve::{decide, verify};
use crate::workflows::{
    fingerprint, mint_note_id, mint_proposal_id, require, NoteKind, NoteSource, ProposalStatus,
    WorkflowError, WorkflowNote, WorkflowProposal, WorkflowStore,
};

/// Everything recorded about a workflow, newest first.
pub fn notes(store: &Arc<dyn WorkflowStore>, id: &str) -> Result<Value, WorkflowError> {
    // Confirm the workflow exists rather than reporting an empty journal for a
    // typo: "this workflow has learned nothing" and "there is no such
    // workflow" are answers a caller acts on very differently.
    require(store.as_ref(), id)?;
    Ok(json!({ "notes": store.list_notes(id)? }))
}

/// Record a note about a workflow.
#[allow(clippy::too_many_arguments)]
pub fn add_note(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    kind: &str,
    text: &str,
    run_ids: Vec<String>,
    source: NoteSource,
    supersedes: Vec<String>,
) -> Result<Value, WorkflowError> {
    require(store.as_ref(), id)?;
    let text = text.trim();
    if text.is_empty() {
        return Err(WorkflowError::Malformed(
            "a note needs some text".to_string(),
        ));
    }
    let kind = parse_kind(kind)?;
    let recorded_at = crate::clock::now_millis() as u64;
    let note = WorkflowNote {
        id: mint_note_id(recorded_at),
        workflow_id: id.to_string(),
        kind,
        text: text.to_string(),
        recorded_at,
        // An operator's own words are pinned, so automation writing a hundred
        // observations cannot evict them when the journal fills.
        pinned: matches!(source, NoteSource::Operator),
        source,
        run_ids,
        superseded_by: None,
    };
    store.append_note(&note)?;

    // Marking what this note replaced is what keeps a journal a set of current
    // claims rather than an argument with itself: a hypothesis a later run
    // disproved stays visible to an operator but stops being briefed.
    let mut superseded = Vec::new();
    for earlier in normalize_supersedes(&note.id, supersedes) {
        if store.supersede_note(id, &earlier, &note.id)? {
            superseded.push(earlier);
        }
    }

    Ok(json!({ "recorded": note.id, "note": note, "superseded": superseded }))
}

/// Keep each predecessor once and never let a note supersede itself.
pub(super) fn normalize_supersedes(current: &str, ids: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ids.into_iter()
        .filter(|id| id != current && seen.insert(id.clone()))
        .collect()
}

/// Every proposal for a workflow, newest first.
pub fn proposals(store: &Arc<dyn WorkflowStore>, id: &str) -> Result<Value, WorkflowError> {
    require(store.as_ref(), id)?;
    Ok(json!({ "proposals": store.list_proposals(id)? }))
}

/// Propose a change, verify it, and store the result.
///
/// Verification happens here rather than at accept time so a proposer learns
/// immediately whether its patch even applies — and so an operator is never
/// offered something that was never checked.
///
/// A proposal that fails verification is still stored, and still returned. That
/// failure is evidence: without it, an agent re-derives the same broken edit
/// every time the same run history comes back around.
pub async fn propose(
    store: &Arc<dyn WorkflowStore>,
    id: &str,
    rationale: &str,
    ops: &Value,
    run_ids: Vec<String>,
    note_ids: Vec<String>,
) -> Result<Value, WorkflowError> {
    let record = require(store.as_ref(), id)?;
    let rationale = rationale.trim();
    if rationale.is_empty() {
        return Err(WorkflowError::Malformed(
            "a proposal needs a rationale: what evidence points to this change".to_string(),
        ));
    }

    let created_at = crate::clock::now_millis() as u64;
    let mut proposal = WorkflowProposal {
        id: mint_proposal_id(created_at),
        workflow_id: id.to_string(),
        created_at,
        rationale: rationale.to_string(),
        ops: ops.clone(),
        evidence_runs: run_ids,
        note_ids,
        base_fingerprint: fingerprint(&record.graph),
        verification: None,
        status: ProposalStatus::Pending,
        decided_at: None,
        decision_reason: None,
    };
    proposal.verification = Some(verify(store, &proposal).await);

    // A workflow with two undecided proposals asks an operator to hold both in
    // their head at once, and the older one was written from less evidence.
    store.save_proposal(&proposal)?;
    if proposal.is_applicable() {
        supersede_earlier(store, &proposal)?;
    }

    Ok(json!({
        "proposed": proposal.id,
        "ok": proposal.is_applicable(),
        "proposal": proposal,
    }))
}

/// Mark this workflow's other undecided proposals as stale.
///
/// Stale rather than rejected: nobody disagreed with them, they were simply
/// replaced by a better-informed one.
fn supersede_earlier(
    store: &Arc<dyn WorkflowStore>,
    keeping: &WorkflowProposal,
) -> Result<(), WorkflowError> {
    for mut earlier in store.list_proposals(&keeping.workflow_id)? {
        if earlier.id == keeping.id || !earlier.is_pending() {
            continue;
        }
        earlier.status = ProposalStatus::Stale;
        earlier.decided_at = Some(keeping.created_at);
        earlier.decision_reason = Some(format!("replaced by proposal {}", keeping.id));
        store.save_proposal(&earlier)?;
    }
    Ok(())
}

/// Apply a proposal to the saved graph.
pub fn accept_proposal(
    store: &Arc<dyn WorkflowStore>,
    proposal_id: &str,
) -> Result<Value, WorkflowError> {
    let (record, proposal) = decide::accept(store, proposal_id)?;
    Ok(json!({
        "accepted": proposal.id,
        "workflow": record_value(&record),
    }))
}

/// Turn a proposal down, recording why.
pub fn reject_proposal(
    store: &Arc<dyn WorkflowStore>,
    proposal_id: &str,
    reason: &str,
) -> Result<Value, WorkflowError> {
    let proposal = decide::reject(store, proposal_id, reason)?;
    Ok(json!({ "rejected": proposal.id, "proposal": proposal }))
}

/// Re-check a proposal against the graph as it stands now.
///
/// Worth having as its own verb because a proposal's verification ages: the
/// graph it was checked against may have moved, and an operator deciding today
/// wants today's answer.
pub async fn verify_proposal(
    store: &Arc<dyn WorkflowStore>,
    proposal_id: &str,
) -> Result<Value, WorkflowError> {
    let mut proposal = crate::workflows::require_proposal(store.as_ref(), proposal_id)?;
    let current = require(store.as_ref(), &proposal.workflow_id)?;
    if fingerprint(&current.graph) != proposal.base_fingerprint {
        proposal.status = ProposalStatus::Stale;
        proposal.decided_at = Some(crate::clock::now_millis() as u64);
        proposal.decision_reason =
            Some("the workflow changed after this proposal was written".to_string());
        store.save_proposal(&proposal)?;
        return Ok(json!({ "ok": false, "proposal": proposal }));
    }
    proposal.verification = Some(verify(store, &proposal).await);
    store.save_proposal(&proposal)?;
    Ok(json!({ "ok": proposal.is_applicable(), "proposal": proposal }))
}

/// The note kinds, as an author spells them.
fn parse_kind(kind: &str) -> Result<NoteKind, WorkflowError> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "observation" => Ok(NoteKind::Observation),
        "hypothesis" => Ok(NoteKind::Hypothesis),
        "constraint" => Ok(NoteKind::Constraint),
        "fix" => Ok(NoteKind::Fix),
        "rejection" => Ok(NoteKind::Rejection),
        other => Err(WorkflowError::Malformed(format!(
            "unknown note kind '{other}'; expected one of: observation, hypothesis, \
             constraint, fix, rejection"
        ))),
    }
}

/// Review a workflow on this machine, for real.
///
/// Starts an embedded host and dispatches one review turn. The system note is
/// written whether or not that turn produces anything, so even a pass whose
/// harness is missing leaves the workflow knowing more than it did.
pub async fn evolve(
    store: &Arc<dyn WorkflowStore>,
    config: &crate::config::WorkflowsConfig,
    cwd: &std::path::Path,
    id: &str,
    run_id: Option<&str>,
) -> Result<Value, WorkflowError> {
    use crate::workflows::evolve::EvolveTrigger;

    let trigger = match run_id {
        Some(run_id) => EvolveTrigger::Failure(run_id.to_string()),
        None => EvolveTrigger::Manual,
    };
    let outcome =
        crate::workflows::local::evolve_here(store.clone(), config, cwd, id, trigger).await?;
    Ok(json!({
        "skipped": outcome.skipped,
        "reply": outcome.reply,
        "notes": outcome.notes,
        "proposals": outcome.proposals,
    }))
}
