//! Medulla's authoring gates: the engine's, plus the one about harnesses.
//!
//! The gates that are true on any host — a prompt written as a `=`-expression,
//! a binding that reads through the wrong shape, a `code` node naming a language
//! the engine does not distinguish — live in [`tinyflows::gates`] now. They were
//! never about Medulla, and every host embedding the engine loses the same
//! afternoons to the same graphs.
//!
//! What is left here is the gate that *needs* this host's vocabulary: which
//! harnesses exist, and the refusal of a harness chosen by a `=`-expression.
//! [`failures`] runs both, and [`MedullaPolicy`] is how a store applies them to
//! every write.

mod harness;

/// The harness-choice gate alone, re-exported so a run boundary can re-check
/// it against a persisted graph that may never have passed through an
/// authoring write — see [`crate::workflows::run`]'s use of it.
pub use harness::failures as harness_failures;

use tinyflows::model::WorkflowGraph;
use tinyflows::store::{gate_failures_into_error, HostPolicy, WorkflowDefaults};

use crate::workflows::WorkflowError;

/// Run every gate, collecting all failures rather than the first.
///
/// One round trip then tells an author everything wrong with what they wrote,
/// which matters most when the author is an agent editing over a tool call.
///
/// # Errors
///
/// Returns [`WorkflowError::Invalid`] listing every failure. An empty result is
/// a pass.
pub fn check(id: &str, graph: &WorkflowGraph) -> Result<(), WorkflowError> {
    gate_failures_into_error(id, failures(graph))
}

/// Every gate failure in `graph`: the engine's, then this host's.
#[must_use]
pub fn failures(graph: &WorkflowGraph) -> Vec<String> {
    let mut failures = tinyflows::gates::failures(graph);
    failures.extend(harness::failures(graph));
    failures
}

/// The rules Medulla judges a workflow document and an authoring write by.
///
/// Both halves exist for the same reason: the engine holds `defaults.harness`
/// and an `agent` node's `harness` as opaque strings, because which harnesses
/// exist is this host's vocabulary and not the engine's. A store carrying this
/// policy refuses a bad one at the boundary — at load, and before a write lands
/// — rather than minutes into a run, after the steps before it have had their
/// effects.
#[derive(Debug, Clone, Copy, Default)]
pub struct MedullaPolicy;

impl HostPolicy for MedullaPolicy {
    fn check_defaults(&self, defaults: &WorkflowDefaults) -> Result<(), String> {
        crate::workflows::store::preference(defaults).map(|_| ())
    }

    fn check_graph(&self, id: &str, graph: &WorkflowGraph) -> Result<(), WorkflowError> {
        // `failures` above, not just the harness gate: overriding this replaces
        // the engine's default wholesale, so dropping `tinyflows::gates` here
        // would silently stop catching everything it catches.
        check(id, graph)
    }
}

#[cfg(test)]
mod tests;
