//! Narrow capability interfaces over the compatibility-facing [`Runtime`].
//!
//! Consumers that only need usage, steering, fleet, or memory can
//! depend on the matching trait instead of the full conversation runtime. A
//! blanket adapter keeps every existing [`Runtime`] implementation compatible.
//!
//! [`Runtime`]: super::Runtime

mod types;

pub use types::{FleetCapability, RuntimeCapabilities, SteeringCapability, UsageCapability};

#[cfg(test)]
mod tests;
