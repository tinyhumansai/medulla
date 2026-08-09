//! The adapter seam between Medulla and the `tinyflows` workflow engine.
//!
//! The engine is deliberately host-agnostic: everything that touches the outside
//! world — running an agent, invoking a tool, making a request, executing code,
//! persisting state, resolving a child workflow — is a trait the embedding
//! application implements. This module is Medulla's implementation of those
//! traits and nothing else.
//!
//! It is kept separate from [`crate::workflows`], which owns what a workflow
//! *is* to this host, so the coupling to a pre-1.0 engine sits in one place. A
//! re-vendor should land here and nowhere else.
//!
//! The module would be called `tinyflows`, to match the sibling `openhuman`
//! host's `src/openhuman/tinyflows/`, but a crate-root module of that name would
//! shadow the `tinyflows` crate in every path inside this crate.
//!
//! The public surface stays narrow: capability assembly, checkpointer opening,
//! and the settings that govern them. Agent prompt evidence is crate-private
//! because it is a run-inspection concern rather than an engine capability.

pub(crate) mod agent_evidence;
pub mod caps;
pub mod execute;
pub mod harness_choice;
pub mod observability;
pub mod settings;

// The capability cases live under `tests/` as submodules once they pushed the
// canonical `tests` file over the 500-line ceiling; see its module doc.
#[cfg(test)]
mod tests;

pub(crate) use caps::build_capabilities_with_agent_evidence;
pub use caps::{build_capabilities, build_dry_run_capabilities, open_checkpointer, HostServices};
pub use execute::{Compiled, Outcome};
pub use harness_choice::{HarnessChoice, HarnessPreference, HarnessSelector};
pub use observability::{
    folding_sink, null_sink, NodeProgressSink, WorkEventSink, WorkflowRunObserver,
};
pub use settings::{
    CapabilitySettings, DEFAULT_MAX_LOOP_ITERATIONS, DEFAULT_MAX_PARALLEL_AGENTS,
    DEFAULT_RUN_TIMEOUT_SECS,
};
