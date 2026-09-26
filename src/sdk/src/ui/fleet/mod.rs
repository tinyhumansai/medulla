//! Pure view-model for the fleet: project the locally registered peers onto the
//! declared containment chain (`Host → Harness → Workspace`) and merge that with
//! whatever the runtime itself declares.
//!
//! This is the counterpart to [`agents`](crate::ui::agents): that view answers
//! "what is running right now", folded from the event stream; this one answers
//! "what exists to run it on", read from what the runtime declares. Neither
//! probes anything, and both degrade to empty rather than to an error.
//!
//! It used to flatten that chain into rows and detail panes as well. Those were
//! the Hosts tab's, and went with it — the capacity snapshot is now read for one
//! thing only: naming the machine and folder a running lane sits in.

mod registry;

#[cfg(test)]
mod tests;

pub use registry::{merge_capacity, registry_capacity};
