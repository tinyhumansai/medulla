//! Narrow capability interfaces over the compatibility-facing [`Runtime`].
//!
//! Consumers that only need usage, steering, fleet, memory, or feedback can
//! depend on the matching trait instead of the full conversation runtime. A
//! blanket adapter keeps every existing [`Runtime`] implementation compatible.
//!
//! [`Runtime`]: super::Runtime

mod types;

pub use types::{
    FeedbackCapability, FleetCapability, MemoryCapability, RuntimeCapabilities, SteeringCapability,
    UsageCapability,
};

#[cfg(test)]
mod tests;
