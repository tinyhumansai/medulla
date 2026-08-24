//! Where workflows and their run records live — Medulla's half of it.
//!
//! The store itself is [`tinyflows::store`] now: the [`WorkflowStore`] trait,
//! the JSON file-backed implementation, the revision snapshots, the journal, the
//! proposal locks, and the record types they move around. None of that was ever
//! about Medulla — a workflow document is the engine's own graph plus
//! bookkeeping, and the sibling hosts that embed the engine need exactly the
//! same bookkeeping.
//!
//! What is left here is the part that *is* about Medulla, and it is two things:
//!
//! - **Where the catalog lives.** [`workflow_dirs`] and [`workspace_state_dir`]
//!   fix the engine's `home`/`project_dir` parameters to Medulla's account
//!   directory and its `.medulla` convention.
//! - **What a `defaults` block may say, and what an edit may contain.** The
//!   engine treats `defaults.harness` and an `agent` node's `harness` as opaque
//!   strings, because which harnesses exist is this host's vocabulary.
//!   [`MedullaPolicy`](crate::workflows::gates::MedullaPolicy) is the rule that
//!   makes a document naming a harness Medulla does not have fail at load, and
//!   an edit introducing one fail before it is written.
//!
//! Everything else is re-exported, so a call site in this crate still writes
//! `crate::workflows::store::…` and does not have to know which crate the item
//! ended up in.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tinyflows::store::WorkflowDefaults;

pub use tinyflows::store::types::{
    bounded_evidence, bounded_within, fingerprint, record_fingerprint,
};
pub use tinyflows::store::{
    current_notes, mint_note_id, mint_proposal_id, new_run_record, parse_workflow, require,
    require_proposal, require_run, rollback, safe_component, undo_last, validate_graph,
    workspace_scope, workspace_state_dir_under, write_atomic, FileWorkflowStore, LoadReport,
    ProposalDecisionGuard, WorkflowStore, MAX_NOTES, MAX_REVISIONS,
};

use crate::home::medulla_home;
use crate::workflows::gates::MedullaPolicy;

/// The per-checkout directory Medulla keeps its own data in.
///
/// A repository's `.medulla/workflows` is read as a source of
/// repository-provided defaults; it is never written to.
pub const PROJECT_DIR: &str = ".medulla";

/// The workflow directories, lowest precedence first: project-local
/// `<cwd>/.medulla/workflows`, then user-global `<medulla home>/workflows`.
///
/// The two are always distinct directories. They used to be able to collapse
/// into one — under `MEDULLA_DEV=1` the home *was* `./.medulla`, and reading it
/// twice made every workflow shadow itself — but the home is now the account
/// directory one level inside the root (`./.medulla/<account>`), which no
/// project store can name.
pub fn workflow_dirs(env: &HashMap<String, String>, cwd: &Path) -> Vec<PathBuf> {
    tinyflows::store::workflow_dirs(&medulla_home(env), cwd, PROJECT_DIR)
}

/// The host-state directory this environment and working directory resolve to.
///
/// Everything Medulla records *about* workflows rather than as part of them —
/// runs, journal notes, proposals, copilot transcripts — hangs off this one
/// path, scoped to the workspace so two checkouts of the same repository do not
/// read each other's history.
pub fn workspace_state_dir(env: &HashMap<String, String>, cwd: &Path) -> PathBuf {
    workspace_state_dir_under(&medulla_home(env), cwd)
}

/// A `defaults` block as the preference layer the dispatch path understands.
///
/// Lives here rather than on [`WorkflowDefaults`] because
/// [`HarnessPreference`](crate::flow_engine::HarnessPreference) is Medulla's
/// type and the record is the engine's.
///
/// # Errors
///
/// Returns a sentence when `harness` names something that cannot be a harness.
pub fn preference(
    defaults: &WorkflowDefaults,
) -> Result<crate::flow_engine::HarnessPreference, String> {
    let harness = match defaults.harness.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => Some(crate::flow_engine::HarnessSelector::parse(name)?),
        _ => None,
    };
    Ok(crate::flow_engine::HarnessPreference {
        harness,
        model: defaults
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_string),
    })
}

/// A file-backed store over the conventional Medulla locations, judging every
/// document and every edit by [`MedullaPolicy`].
///
/// The one constructor production code should use: a store built any other way
/// carries the engine's own policy, and would both load a document naming a
/// harness this host cannot reach and let an edit introduce one.
pub fn discover(env: &HashMap<String, String>, cwd: &Path) -> FileWorkflowStore {
    let state_dir = medulla_home(env).join("state").join("workflows");
    with_medulla_policy(FileWorkflowStore::with_workspace_state(
        workflow_dirs(env, cwd),
        &state_dir,
        cwd,
    ))
}

/// Attach [`MedullaPolicy`] to a store built over explicit directories.
///
/// For the callers that choose their own layout — tests, and the harnesses that
/// run a workflow out of a scratch directory. Without it such a store silently
/// holds a laxer policy than the discovered one, so an edit that production
/// would refuse passes.
#[must_use]
pub fn with_medulla_policy(store: FileWorkflowStore) -> FileWorkflowStore {
    store.with_policy(Arc::new(MedullaPolicy))
}
