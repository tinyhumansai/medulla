//! Authored, durable, multi-step work: workflow definitions and their runs.
//!
//! A workflow is a directed acyclic graph of typed nodes — trigger, agent, tool
//! call, HTTP request, condition, merge, sub-workflow — executed by the
//! [`tinyflows`] engine. Where a Medulla task is one instruction handed to one
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
//! Definitions are JSON documents in the layered workflow directories
//! (`<medulla home>/workflows`, then `<cwd>/.medulla/workflows`), the same
//! layering agent templates use. The store is behind the [`WorkflowStore`]
//! trait, so a remote catalog is a new implementation rather than a rewrite.

pub mod authoring;
pub mod node_contracts;
mod registry;
pub mod run;
pub mod store;
mod types;

#[cfg(test)]
mod tests;

pub use authoring::{
    apply_workflow_ops, create_workflow, preview_workflow_ops, validate_handle, GraphHandle,
};
pub use node_contracts::{all_node_kind_contracts, node_kind_contract};
pub use registry::StoreWorkflowResolver;
pub use run::{dry_run, resume_workflow, run_workflow, RunContext};
pub use store::{
    new_run_record, parse_workflow, require, require_run, validate_graph, FileWorkflowStore,
    LoadReport, WorkflowStore,
};
pub use types::{
    RunId, RunRecord, RunStatus, RunStep, WorkflowError, WorkflowId, WorkflowRecord,
    WorkflowSummary,
};
