//! Authored, durable, multi-step work: workflow definitions and their runs.
//!
//! A workflow is a directed graph of typed nodes — trigger, agent, tool call,
//! HTTP request, condition, merge, loop, sub-workflow — executed by the
//! [`tinyflows`] engine. Usually acyclic; a `loop` node repeats a section a
//! bounded number of times, and the edge closing that cycle is the one place a
//! workflow runs backwards. Where a Medulla task is one instruction handed to one
//! harness, a workflow is a plan an operator or an agent can write down, review,
//! version, and re-run.
//!
//! This module owns the *domain*: what a workflow is to this host, where it is
//! stored, and how a run is recorded. The adapter that teaches the engine to
//! reach Medulla's harness fleet lives next door in [`crate::flow_engine`], and
//! the two are kept apart on purpose — this module should read as Medulla code,
//! not as engine glue. (That module would be called `tinyflows` to match the
//! sibling `openhuman` host, but a crate-root module of that name would collide
//! with the `tinyflows` crate itself in every path inside this crate.)
//!
//! Definitions are JSON documents under `<medulla home>/workflows`.
//! Repository-provided defaults under `<cwd>/.medulla/workflows` remain
//! readable, but user-authored changes overlay them from the home directory.
//! The store is behind the [`WorkflowStore`] trait, so a remote catalog is a new
//! implementation rather than a rewrite.

pub mod bridge;
pub mod copilot;
mod dispatch_error;
pub mod evolve;
pub mod gates;
pub mod local;
pub(crate) mod mcp;
pub mod node_contracts;
pub mod ops;
mod registry;
pub mod report;
pub mod run;
pub mod skills;
pub mod store;
pub mod workspace;

#[cfg(test)]
#[path = "authoring_tests.rs"]
mod authoring_tests;
#[cfg(test)]
mod tests;

// Patch-based editing moved to the engine crate with the store it writes to;
// the gates an edit is judged by ride along on the store's own policy.
pub use bridge::{cancel_task_workflow, run_task_workflow, StoreWorkflowBridge};
pub use copilot::{CopilotOutcome, CopilotRequest, CopilotSession, FailedRun};
pub use local::{LocalCopilotDispatch, LocalWorkflowHost, LOCAL_WORKER_ADDRESS};
pub use node_contracts::{all_node_kind_contracts, node_kind_contract};
pub use ops::discover_store;
pub use registry::StoreWorkflowResolver;
pub use report::RunReporter;
pub use run::{dry_run, resume_workflow, run_workflow, run_workflow_versioned, RunContext};
pub use store::{
    bounded_evidence, bounded_within, current_notes, mint_note_id, mint_proposal_id,
    new_run_record, parse_workflow, require, require_proposal, require_run, rollback, undo_last,
    validate_graph, LoadReport, WorkflowStore, MAX_NOTES, MAX_REVISIONS,
};
pub use tinyflows::store::{
    apply_workflow_ops, apply_workflow_ops_if_unchanged, create_workflow, preview_workflow_ops,
    validate_handle, GraphHandle,
};
// The engine's own graph model, re-exported so hosts above this crate (the TUI)
// can name a workflow's graph without taking a direct dependency on the engine.
// The type is the shared contract, not a Medulla type, so re-exporting it is the
// alternative to a parallel copy that would drift. `InputType`/`WorkflowInput`
// come along for the same reason: the TUI has to render and coerce a declared
// input, and a second declaration of that shape would be free to disagree with
// the engine's about what a `number` accepts.
pub use tinyflows::model::{InputType, WorkflowGraph, WorkflowInput};
// The stored model moved to the engine crate with the store that persists it.
// Re-exported unchanged so a call site in this crate — and the TUI above it —
// still writes `crate::workflows::WorkflowRecord`.
pub use gates::MedullaPolicy;
pub use store::{fingerprint, record_fingerprint};
pub use store::{preference as defaults_preference, with_medulla_policy};
// Aliased as well as re-exported, so `workflows::authoring::…` — the path this
// crate already used everywhere — keeps resolving after the move.
pub use tinyflows::store::authoring;
pub use tinyflows::store::{
    Diagnosis, FileWorkflowStore, NoteId, NoteKind, NoteSource, ProposalId, ProposalStatus,
    ProposalVerification, RunExecutor, RunId, RunOrigin, RunRecord, RunStatus, RunStep,
    TranscriptEntry,
    WorkflowDefaults, WorkflowError, WorkflowId, WorkflowNote, WorkflowProposal, WorkflowRecord,
    WorkflowRevision, WorkflowSummary,
};
