//! Process ownership for agent launches managed by Medulla.

mod registry;

#[cfg(unix)]
pub use registry::current_process_identity;
pub use registry::{
    agent_registry, AgentRegistration, AgentRegistry, RegistrationGuard, RegistrationHandle,
};

#[cfg(test)]
mod registry_tests;
