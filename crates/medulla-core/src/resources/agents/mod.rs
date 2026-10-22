//! Platform-neutral coding-agent process discovery and classification.

mod classify;
mod registry;
mod types;

pub use classify::{discover_agents, discover_agents_with_options};
pub use registry::{discover_registered_agents, registered_roots};
pub use types::{
    AgentDiscoveryOptions, AgentKind, AgentOrigin, AgentProcess, IoCounters, KnownProcessRoot,
    ProcessNode, ProcessRecord, ProcessUsage,
};

#[cfg(test)]
mod tests;
