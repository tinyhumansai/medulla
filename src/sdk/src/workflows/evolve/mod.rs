//! Workflows that learn from their own history.
//!
//! A workflow could previously say what it *is* and what one run *did*, but
//! nothing carried across runs. Every diagnosis started from zero, and a
//! conclusion reached last week was gone by the time it mattered again.
//!
//! An *evolution pass* closes that loop:
//!
//! 1. **Observe.** A failed run always writes a system note, built from the run
//!    record alone. No dispatch, so it works everywhere — including the CLI,
//!    where there is no harness to ask.
//! 2. **Reason.** The pass hands an agent the workflow's current notes, its
//!    recent runs, and the failure, with a tool set that cannot edit a graph.
//! 3. **Propose.** Any change the agent wants becomes a [`WorkflowProposal`],
//!    checked by [`verify`] against the live graph.
//! 4. **Decide.** An operator accepts or rejects. [`decide::accept`] is the
//!    only path in this feature that touches a saved workflow.
//!
//! The boundary in step 2 is enforced at the MCP layer rather than in the
//! prompt. A standing instruction not to edit is a request; a tool that is not
//! served is a fact, and one confused turn should not be able to rewrite a
//! graph nobody was watching.

mod context;
pub mod decide;
mod registry;
pub(crate) mod session;
mod types;
mod verify;

#[cfg(test)]
mod tests;

pub use context::{failing_nodes, record_failure_note};
pub use decide::{accept, reject};
pub use registry::{is_evolving, EvolveGuard};
pub use session::EvolveSession;
pub use types::{EvolveConfig, EvolveOutcome, EvolveTrigger};
pub use verify::verify;
