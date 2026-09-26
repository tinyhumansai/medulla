//! Driving the engine: compile, run, resume, simulate.
//!
//! These four functions are the whole of how this host executes a graph, and
//! they exist so the *domain* never names an engine entry point. The engine's
//! `engine::*` surface is the least stable part of a pre-1.0 crate — twenty-odd
//! near-duplicate `run_*`/`resume_*` overloads that will plausibly collapse into
//! an options struct — so keeping every call to it in one file is what makes a
//! re-vendor a one-file problem rather than a search-and-replace.
//!
//! The graph *data model* is deliberately not wrapped. `WorkflowGraph`, `Node`,
//! and `GraphOp` are the portable contract shared with the sibling hosts and
//! with anything reading a workflow document; re-declaring them here would only
//! create a second vocabulary to keep in sync.

use std::sync::Arc;

use serde_json::Value;
use tinyflows::engine::{
    resume_with_checkpointer_journaled_observed, run_with_checkpointer_journaled_observed,
    Checkpointer, InMemoryGraphEventJournal,
};
use tinyflows::model::WorkflowGraph;
use tinyflows::observability::RunObserver;

/// A graph that passed validation and is ready to run.
pub struct Compiled(tinyflows::compiler::CompiledWorkflow);

/// How a run ended, in this host's terms.
///
/// Mirrors the engine's outcome rather than re-exporting it, so a field added
/// upstream is a change here and nowhere else.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// The final run state: every node's output, keyed by node id.
    pub output: Value,
    /// Node ids that parked the run awaiting approval. Empty for a run that
    /// finished.
    pub pending_approvals: Vec<String>,
    /// Whether the engine wound down early on a cancellation.
    pub cancelled: bool,
}

impl From<tinyflows::engine::RunOutcome> for Outcome {
    fn from(outcome: tinyflows::engine::RunOutcome) -> Self {
        Self {
            output: outcome.output,
            pending_approvals: outcome.pending_approvals,
            cancelled: outcome.cancelled,
        }
    }
}

/// Compile `graph`, returning the engine's own message on failure.
pub fn compile(graph: &WorkflowGraph) -> Result<Compiled, String> {
    tinyflows::compiler::compile(graph)
        .map(Compiled)
        .map_err(|err| err.to_string())
}

/// Run `compiled` to completion, an approval gate, or a failure.
///
/// Checkpointed and observed: the checkpoint is what lets an approval pause
/// outlive the process, and the observer is what reports progress. The journal
/// is in-memory because this host reads progress from the observer rather than
/// replaying events afterwards.
///
/// `input` is `impl Into<RunInput>`, so a caller passes either a bare [`Value`]
/// (the trigger payload alone) or a `RunInput` carrying values for the graph's
/// declared inputs as well. The engine validates those against the graph before
/// anything executes and fails the call if they do not satisfy it.
pub async fn run(
    compiled: &Compiled,
    input: impl Into<tinyflows::engine::RunInput>,
    capabilities: &tinyflows::caps::Capabilities,
    checkpointer: Arc<dyn Checkpointer<Value>>,
    thread_id: &str,
    observer: &Arc<dyn RunObserver>,
) -> Result<Outcome, String> {
    run_with_checkpointer_journaled_observed(
        &compiled.0,
        input,
        capabilities,
        checkpointer,
        thread_id,
        Arc::new(InMemoryGraphEventJournal::default()),
        observer,
    )
    .await
    .map(|journaled| journaled.outcome.into())
    .map_err(|err| err.to_string())
}

/// Continue a run that parked on approval gates.
pub async fn resume(
    compiled: &Compiled,
    capabilities: &tinyflows::caps::Capabilities,
    checkpointer: Arc<dyn Checkpointer<Value>>,
    thread_id: &str,
    approvals: Vec<String>,
    rejections: Vec<String>,
    observer: &Arc<dyn RunObserver>,
) -> Result<Outcome, String> {
    resume_with_checkpointer_journaled_observed(
        &compiled.0,
        capabilities,
        checkpointer,
        thread_id,
        approvals,
        rejections,
        Arc::new(InMemoryGraphEventJournal::default()),
        observer,
    )
    .await
    .map(|journaled| journaled.outcome.into())
    .map_err(|err| err.to_string())
}

/// Run `compiled` to completion off the caller's super-step.
///
/// The entry point for a child graph a `spawn` node started. It is deliberately
/// neither checkpointed nor observed: a spawned child has no thread id of its
/// own, so there is nothing for a resume to address and nothing watching it but
/// the `gate` that will collect its ticket. A child that parks on an approval
/// therefore returns with `pending_approvals` set rather than waiting for one —
/// the honest answer, since nobody can approve a run they cannot name.
pub async fn run_detached(
    compiled: &Compiled,
    input: impl Into<tinyflows::engine::RunInput>,
    capabilities: &tinyflows::caps::Capabilities,
) -> Result<Outcome, String> {
    tinyflows::engine::run(&compiled.0, input, capabilities)
        .await
        .map(Outcome::from)
        .map_err(|err| err.to_string())
}

/// Run `compiled` against capabilities that touch nothing.
///
/// Not checkpointed: a simulation has no state worth keeping.
pub async fn simulate(
    compiled: &Compiled,
    input: impl Into<tinyflows::engine::RunInput>,
    capabilities: &tinyflows::caps::Capabilities,
) -> Result<Outcome, String> {
    tinyflows::engine::run(&compiled.0, input, capabilities)
        .await
        .map(Outcome::from)
        .map_err(|err| err.to_string())
}

/// Simulate `compiled`, reporting every step to `observer`.
///
/// The observed variant exists because a simulation's *steps* are the whole
/// point of running one at authoring time. The outcome says the graph completed;
/// only the per-step record says which expressions resolved to null on the way,
/// and a binding that silently resolved null is the failure a dry run is for.
pub async fn simulate_observed(
    compiled: &Compiled,
    input: impl Into<tinyflows::engine::RunInput>,
    capabilities: &tinyflows::caps::Capabilities,
    observer: &Arc<dyn RunObserver>,
) -> Result<Outcome, String> {
    tinyflows::engine::run_with_observer(&compiled.0, input, capabilities, observer)
        .await
        .map(Outcome::from)
        .map_err(|err| err.to_string())
}
