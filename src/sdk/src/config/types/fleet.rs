//! Operator-declared fleet capacity and its UI-facing projections.

use super::*;

/// Operator-declared capacity: the `Host → Harness → Workspace → Agent`
/// containment chain plus the agent templates that may be provisioned into it.
///
/// Declared, never probed — this is what the client *offers* the orchestrator
/// when it attaches to `medulla-serve`, and what the TUI's Fleet page renders
/// when no backend supplies a fleet of its own. The default declares only the
/// built-in coding template catalog; it provisions no agents and advertises no
/// host capacity. An explicit empty template list opts out of that catalog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct FleetConfig {
    /// Machines this client declares.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<HostDescriptor>,
    /// Agent CLI runtimes installed on those machines.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<HarnessDescriptor>,
    /// Folders those runtimes expose.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<WorkspaceDescriptor>,
    /// Durable agent identities deployed into them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<AgentDescriptor>,
    /// Kinds of agent that may be provisioned onto this chain.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agent_templates: Vec<AgentTemplate>,
}

impl Default for FleetConfig {
    fn default() -> Self {
        Self {
            hosts: Vec::new(),
            harnesses: Vec::new(),
            workspaces: Vec::new(),
            agents: Vec::new(),
            agent_templates: crate::agents::default_templates(),
        }
    }
}

impl FleetConfig {
    /// Whether the operator declared nothing at all.
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
            && self.harnesses.is_empty()
            && self.workspaces.is_empty()
            && self.agents.is_empty()
            && self.agent_templates.is_empty()
    }

    /// Whether a catalog is all this config declares — no hosts, harnesses,
    /// workspaces, or agents.
    ///
    /// A template catalog says what *may* be provisioned, never where. So a
    /// config carrying only one has declared no fleet, and the opt-in demo
    /// fleet may still stand in: this is what lets the built-in catalog (or an
    /// installed `.medulla/agents` store) coexist with `MEDULLA_DEMO_FLEET`
    /// instead of suppressing it.
    pub fn declares_only_templates(&self) -> bool {
        self.hosts.is_empty()
            && self.harnesses.is_empty()
            && self.workspaces.is_empty()
            && self.agents.is_empty()
    }

    /// The declared chain as the UI-facing roll-up (agents excluded — they reach
    /// the UI through the snapshot roster).
    pub fn capacity(&self) -> CapacitySnapshot {
        CapacitySnapshot {
            hosts: self.hosts.clone(),
            harnesses: self.harnesses.clone(),
            workspaces: self.workspaces.clone(),
            templates: self.agent_templates.clone(),
        }
    }
}
