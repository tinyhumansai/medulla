//! The declared-capacity contracts: the strict single-parent containment chain
//! `Host → Harness → Workspace → Agent`, the agent-template catalog that
//! constrains what may be provisioned into it, and the [`CapacitySnapshot`]
//! roll-up the UI renders.
//!
//! Everything here is *declared, never probed*: a record is what an operator or
//! a manager said exists, not a live measurement. Parsing is deliberately
//! forgiving — a descriptor whose parent link is unknown still renders, under
//! the id it claims, rather than vanishing — because a half-synced fleet must
//! never blank the view.
//!
//! These types were previously private to the unix-only `medulla-serve`
//! runtime. They live here because the same shapes now reach every runtime and
//! the whole UI layer, neither of which is unix-only.
//!
//! [`demo`] holds the env-gated stand-in fleet used to exercise the UI without a
//! backend; nothing in this module reads it. The default agent-template catalog
//! and its on-disk store live in [`crate::agents`], which owns everything about
//! where templates come from.

use serde::{Deserialize, Serialize};
use serde_json::Value;

mod demo;
pub use demo::{
    demo_agents, demo_capacity, demo_fleet_requested, demo_requested_from, DEMO_FLEET_ENV,
};

/// Resources declared for one host in the manager placement graph.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostResources {
    /// Logical CPU cores available to the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_cores: Option<f64>,
    /// Total physical memory in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_memory_bytes: Option<u64>,
    /// Currently available physical memory in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_memory_bytes: Option<u64>,
    /// Currently available disk capacity in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_free_bytes: Option<u64>,
    /// One-minute load average, as reported by the host. Divided by
    /// [`cpu_cores`](HostResources::cpu_cores) it is the machine's utilisation;
    /// on its own it means nothing, which is why both travel together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_average_1m: Option<f64>,
}

/// A machine on which one or more harnesses run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostDescriptor {
    /// Stable manager-scoped host identifier.
    pub id: String,
    /// Operator-facing host name.
    pub name: String,
    /// Current manager-reported availability.
    pub availability: String,
    /// Network address used to reach the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Latest resource observation for the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<HostResources>,
    /// Deployment-specific host attributes.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// A token budget declared on a harness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HarnessBudget {
    /// Provider that meters the budget.
    pub provider: String,
    /// Provider-defined accounting window.
    pub window: String,
    /// Optional provider seat or account identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    /// Total token allowance in the current window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_tokens: Option<u64>,
    /// Tokens consumed in the current window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_tokens: Option<u64>,
    /// Tokens still available in the current window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_tokens: Option<u64>,
    /// Unix timestamp after which a depleted budget becomes usable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown_until: Option<u64>,
    /// Origin of the budget observation.
    pub source: String,
}

/// An installed agent runtime on a host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HarnessDescriptor {
    /// Stable manager-scoped harness identifier.
    pub id: String,
    /// Identifier of the host running this harness.
    pub host_id: String,
    /// Harness implementation or protocol family.
    pub kind: String,
    /// Current manager-reported availability.
    pub availability: String,
    /// Whether the harness is ready to accept work.
    pub ready: bool,
    /// Operator-facing explanation when the harness is not ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_reason: Option<String>,
    /// Inference providers exposed by the harness.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    /// Agent templates the harness can instantiate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub template_ids: Vec<String>,
    /// Provider budgets currently visible to the harness.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub budgets: Vec<HarnessBudget>,
    /// Deployment-specific harness attributes.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Parsed, advisory `MEDULLA.md` profile attached to a workspace.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProfile {
    /// Advisory instructions parsed from `MEDULLA.md`.
    #[serde(default)]
    pub instructions: String,
    /// Harness kinds preferred by the workspace.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<String>,
    /// Named model preferences declared by the workspace.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub models: serde_json::Map<String, Value>,
    /// Additional profile properties preserved from the declaration.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// A folder exposed by exactly one harness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDescriptor {
    /// Stable manager-scoped workspace identifier.
    pub id: String,
    /// Operator-facing workspace name.
    pub name: String,
    /// Filesystem path as seen by the owning harness.
    pub path: String,
    /// Identifier of the harness that exposes this workspace.
    pub harness_id: String,
    /// Parsed advisory workspace profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<WorkspaceProfile>,
    /// Project or repository identity, when discovered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Agent templates explicitly enabled for this workspace.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub template_ids: Vec<String>,
    /// Deployment-specific workspace attributes.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Harness-specific overrides for one agent template.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentTemplateHarnessOverride {
    /// Harness-specific model selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Harness-specific reasoning effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Harness-specific tool allowlist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Harness-specific provider parameters.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub params: serde_json::Map<String, Value>,
    /// Harness-specific instruction replacement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Operator-declared kind of agent that a manager may provision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentTemplate {
    /// Stable manager-scoped template identifier.
    pub id: String,
    /// Optional operator-facing template name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Operator-facing description of the agent role.
    pub description: String,
    /// Default instruction text supplied to instantiated agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Default tool allowlist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Default model selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Default reasoning effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Default provider-specific parameters.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub params: serde_json::Map<String, Value>,
    /// Discovery and routing labels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Deployment-specific template attributes.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
    /// Per-harness overrides keyed by harness kind.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub harnesses: std::collections::BTreeMap<String, AgentTemplateHarnessOverride>,
}

/// The declared capacity a runtime currently knows about: the containment chain
/// and the template catalog, with the agent level supplied separately by
/// [`RuntimeSnapshot::roster`](crate::runtime::RuntimeSnapshot::roster).
///
/// Empty is the normal resting state — a backend that declares nothing yields a
/// snapshot byte-identical to one taken before capacity existed, and every
/// consumer degrades to "nothing declared" rather than to an error.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct CapacitySnapshot {
    /// Machines that run harnesses.
    pub hosts: Vec<HostDescriptor>,
    /// Agent CLI runtimes installed on those hosts.
    pub harnesses: Vec<HarnessDescriptor>,
    /// Folders exposed by those harnesses.
    pub workspaces: Vec<WorkspaceDescriptor>,
    /// Kinds of agent that may be provisioned into the chain.
    pub templates: Vec<AgentTemplate>,
}

impl CapacitySnapshot {
    /// Whether nothing at all is declared. The UI hides its fleet surfaces
    /// entirely in that case rather than rendering an empty tree.
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
            && self.harnesses.is_empty()
            && self.workspaces.is_empty()
            && self.templates.is_empty()
    }

    /// The workspace with this id, if declared.
    pub fn workspace(&self, id: &str) -> Option<&WorkspaceDescriptor> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    /// The harness with this id, if declared.
    pub fn harness(&self, id: &str) -> Option<&HarnessDescriptor> {
        self.harnesses.iter().find(|h| h.id == id)
    }

    /// The host with this id, if declared.
    pub fn host(&self, id: &str) -> Option<&HostDescriptor> {
        self.hosts.iter().find(|h| h.id == id)
    }

    /// The template with this id, if declared.
    pub fn template(&self, id: &str) -> Option<&AgentTemplate> {
        self.templates.iter().find(|t| t.id == id)
    }

    /// Walk an agent up the containment chain to its declared placement.
    ///
    /// A workspace-backed agent derives its harness and host by walking up; a
    /// local agent (no `workspaceId`) may name a `hostId` directly instead. A
    /// dangling parent link resolves to whatever levels *are* declared rather
    /// than to nothing, so a half-synced fleet still explains where an agent is.
    pub fn placement(&self, agent: &crate::runtime::AgentDescriptor) -> AgentPlacement<'_> {
        let workspace = agent
            .workspace_id
            .as_deref()
            .and_then(|id| self.workspace(id));
        let harness = workspace.and_then(|w| self.harness(&w.harness_id));
        let host = match harness {
            Some(h) => self.host(&h.host_id),
            None => agent.host_id.as_deref().and_then(|id| self.host(id)),
        };
        AgentPlacement {
            host,
            harness,
            workspace,
            template: agent
                .template_id
                .as_deref()
                .and_then(|id| self.template(id)),
        }
    }
}

/// One agent's resolved position in the containment chain. Every level is
/// optional: a local agent has no workspace, and any link may dangle.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentPlacement<'a> {
    /// The machine, derived through the harness or declared directly.
    pub host: Option<&'a HostDescriptor>,
    /// The agent CLI runtime exposing the workspace.
    pub harness: Option<&'a HarnessDescriptor>,
    /// The folder the agent is deployed into.
    pub workspace: Option<&'a WorkspaceDescriptor>,
    /// The template the agent was provisioned from.
    pub template: Option<&'a AgentTemplate>,
}

impl HarnessBudget {
    /// Remaining tokens as a fraction of the window limit, when both are known.
    pub fn fraction_remaining(&self) -> Option<f64> {
        let limit = self.limit_tokens.filter(|l| *l > 0)? as f64;
        let remaining = match (self.remaining_tokens, self.used_tokens) {
            (Some(r), _) => r as f64,
            (None, Some(u)) => (limit - u as f64).max(0.0),
            (None, None) => return None,
        };
        Some((remaining / limit).clamp(0.0, 1.0))
    }

    /// Tokens still available, preferring the reported remainder and falling
    /// back to `limit - used`.
    pub fn remaining(&self) -> Option<u64> {
        match (self.remaining_tokens, self.limit_tokens, self.used_tokens) {
            (Some(r), _, _) => Some(r),
            (None, Some(l), Some(u)) => Some(l.saturating_sub(u)),
            _ => None,
        }
    }
}
