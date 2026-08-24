//! The data model for stored workflows, their runs, and what a host learns
//! about them.
//!
//! A *workflow* is a [`tinyflows::model::WorkflowGraph`] — the engine's own
//! portable JSON shape — plus the bookkeeping this host needs to find it, list
//! it, and say where it came from. The graph itself is deliberately not
//! re-modelled here: it is the contract shared with the engine and with the
//! sibling hosts that embed it, and a parallel Medulla-side copy would only
//! drift.
//!
//! Runs are recorded rather than merely streamed, so a workflow that paused for
//! approval or died with the process can be found again by id.
//!
//! The submodules split the model by lifetime rather than by shape, because
//! that is what decides where each type is stored:
//!
//! - [`workflow`] — the versioned document an operator edits.
//! - [`run`] — one execution's durable record, written once and never revised.
//! - [`note`] — what the host has learned about a workflow across runs.
//! - [`proposal`] — a graph change suggested but not yet made.
//! - [`error`] — the failure vocabulary every surface reports through.

mod error;
mod note;
mod proposal;
mod run;
mod workflow;

#[cfg(test)]
mod tests;

pub use error::WorkflowError;
pub use note::{NoteId, NoteKind, NoteSource, WorkflowNote};
pub use proposal::{
    fingerprint, ProposalId, ProposalStatus, ProposalVerification, WorkflowProposal,
};
pub(crate) use run::{bounded_evidence, bounded_within};
pub use run::{RunExecutor, RunId, RunOrigin, RunRecord, RunStatus, RunStep};
pub use workflow::{
    record_fingerprint, WorkflowDefaults, WorkflowId, WorkflowRecord, WorkflowRevision,
    WorkflowSummary,
};
