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

#[cfg(test)]
mod tests;

// Split out of `tests` once the `medulla:shell` cases pushed it over the
// repository's 500-line file ceiling — see that file's module doc.
#[cfg(test)]
mod shell_tests;

// Likewise: harness and model selection, asserted on the dispatched task frame.
#[cfg(test)]
mod harness_selection_tests;

pub(crate) use caps::build_capabilities_with_agent_evidence;
pub use caps::{build_capabilities, build_dry_run_capabilities, open_checkpointer, HostServices};
pub use execute::{Compiled, Outcome};
pub use harness_choice::{HarnessChoice, HarnessPreference, HarnessSelector};
pub use observability::{folding_sink, null_sink, WorkEventSink, WorkflowRunObserver};
pub use settings::{CapabilitySettings, DEFAULT_RUN_TIMEOUT_SECS};
