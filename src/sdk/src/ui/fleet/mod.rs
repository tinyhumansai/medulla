//! Pure view-model for the Fleet view: turn the declared capacity
//! (`Host → Harness → Workspace → Agent`) and the agent-template catalog into a
//! flattened row model plus pre-wrapped detail lines.
//!
//! This is the counterpart to [`agents`](crate::ui::agents): that view answers
//! "what is running right now", folded from the event stream; this one answers
//! "what exists to run it on", read from what the runtime declares. Neither
//! probes anything, and both degrade to empty rather than to an error.
//!
//! Split by responsibility: [`types`] holds the node data model, [`rows`] the
//! tree flattening, [`detail`] the selected-node pane, and the private `fmt`
//! submodule the shared magnitude/budget formatters.

mod detail;
mod fmt;
mod rows;
mod types;

#[cfg(test)]
mod tests;

pub use detail::fleet_detail;
pub use rows::fleet_rows;
pub use types::{FleetNode, FleetNodeKind};
