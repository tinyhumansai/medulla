//! An env-gated stand-in fleet: enough declared capacity to exercise every
//! fleet surface with no backend, no socket, and no registered peer.
//!
//! Real capacity arrives from a runtime — a manager's roster, a serve
//! handshake, or the operator's `[fleet]` config — and until one of those is
//! wired the fleet surfaces are correctly, but unhelpfully, empty. This module
//! is the development answer to that: set `MEDULLA_DEMO_FLEET=1` (in the
//! environment or the cwd `.env`) and the UI renders a small, believable fleet
//! instead.
//!
//! Two rules keep it from ever being mistaken for real capacity. It is **opt-in
//! only** — absent the flag nothing here is constructed, so a normal run is
//! byte-identical to one before this module existed. And it **never wins over a
//! real reading**: a runtime that declares anything, or a config that does,
//! takes precedence, so the stand-in cannot mask the thing it stands in for.
//!
//! Every record is deliberately ordinary — two hosts, three harnesses, two
//! workspaces, two agents, two templates — because the point is to see the
//! layout, the budget colouring, and the placement walk, not to stress the caps.

use serde_json::{json, Map};

use super::{
    AgentTemplate, AgentTemplateHarnessOverride, CapacitySnapshot, HarnessBudget,
    HarnessDescriptor, HostDescriptor, HostResources, WorkspaceDescriptor, WorkspaceProfile,
};
use crate::runtime::AgentDescriptor;

/// The environment variable that turns the stand-in fleet on.
pub const DEMO_FLEET_ENV: &str = "MEDULLA_DEMO_FLEET";

/// Whether the operator asked for the stand-in fleet.
///
/// Anything but empty, `0`, `false`, or `no` counts as on, so `=1` and `=true`
/// both work and a stray `MEDULLA_DEMO_FLEET=` does not.
pub fn demo_fleet_requested() -> bool {
    demo_requested_from(std::env::var(DEMO_FLEET_ENV).ok().as_deref())
}

/// Whether `value` — the variable's contents, or `None` when it is unset — asks
/// for the stand-in fleet. Split out from the environment read so the rule is
/// testable without mutating process-global state.
pub fn demo_requested_from(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
    }
}

/// The stand-in containment chain and template catalog.
pub fn demo_capacity() -> CapacitySnapshot {
    CapacitySnapshot {
        hosts: vec![
            HostDescriptor {
                id: "demo-host-workshop".into(),
                name: "workshop".into(),
                availability: "online".into(),
                address: Some("10.0.0.4".into()),
                resources: Some(HostResources {
                    cpu_cores: Some(10.0),
                    total_memory_bytes: Some(32 * GB),
                    available_memory_bytes: Some(11 * GB),
                    disk_free_bytes: Some(220 * GB),
                }),
                metadata: Map::new(),
            },
            HostDescriptor {
                id: "demo-host-builder".into(),
                name: "builder".into(),
                availability: "online".into(),
                address: Some("10.0.0.9".into()),
                resources: Some(HostResources {
                    cpu_cores: Some(4.0),
                    total_memory_bytes: Some(16 * GB),
                    available_memory_bytes: Some(3 * GB),
                    disk_free_bytes: Some(40 * GB),
                }),
                metadata: Map::new(),
            },
        ],
        harnesses: vec![
            HarnessDescriptor {
                id: "demo-harness-claude".into(),
                host_id: "demo-host-workshop".into(),
                kind: "claude-code".into(),
                availability: "online".into(),
                ready: true,
                ready_reason: None,
                providers: vec!["anthropic".into()],
                template_ids: Vec::new(),
                budgets: vec![HarnessBudget {
                    provider: "anthropic".into(),
                    window: "5h".into(),
                    seat: Some("max-20x".into()),
                    limit_tokens: Some(2_000_000),
                    used_tokens: Some(1_240_000),
                    remaining_tokens: Some(760_000),
                    cooldown_until: None,
                    source: "provider_reported".into(),
                }],
                metadata: Map::new(),
            },
            HarnessDescriptor {
                id: "demo-harness-codex".into(),
                host_id: "demo-host-workshop".into(),
                kind: "codex".into(),
                availability: "online".into(),
                ready: true,
                ready_reason: None,
                providers: vec!["openai".into()],
                template_ids: Vec::new(),
                // Nearly spent, so the budget colouring has something to show.
                budgets: vec![HarnessBudget {
                    provider: "openai".into(),
                    window: "weekly".into(),
                    seat: None,
                    limit_tokens: Some(1_000_000),
                    used_tokens: Some(940_000),
                    remaining_tokens: Some(60_000),
                    cooldown_until: None,
                    source: "estimate".into(),
                }],
                metadata: Map::new(),
            },
            HarnessDescriptor {
                id: "demo-harness-opencode".into(),
                host_id: "demo-host-builder".into(),
                kind: "opencode".into(),
                availability: "online".into(),
                ready: false,
                ready_reason: Some("cli not authenticated".into()),
                providers: vec!["openrouter".into()],
                template_ids: Vec::new(),
                budgets: Vec::new(),
                metadata: Map::new(),
            },
        ],
        workspaces: vec![
            WorkspaceDescriptor {
                id: "demo-ws-medulla".into(),
                name: "medulla".into(),
                path: "/srv/repos/medulla".into(),
                harness_id: "demo-harness-claude".into(),
                profile: Some(WorkspaceProfile {
                    instructions: "Prefer small commits. Run the test suite before handing back."
                        .into(),
                    harnesses: vec!["claude-code".into()],
                    models: Map::new(),
                    metadata: Map::new(),
                }),
                project: Some("medulla".into()),
                template_ids: Vec::new(),
                metadata: json!({ "branch": "main" })
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            },
            WorkspaceDescriptor {
                id: "demo-ws-site".into(),
                name: "site".into(),
                path: "/srv/repos/site".into(),
                harness_id: "demo-harness-codex".into(),
                profile: None,
                project: Some("site".into()),
                // Only the reviewer may be stood up here, so the Templates page
                // has a real allowlist to report.
                template_ids: vec!["demo-reviewer".into()],
                metadata: Map::new(),
            },
        ],
        templates: vec![
            AgentTemplate {
                id: "demo-implementer".into(),
                name: Some("Implementer".into()),
                description: "Implements a scoped change and reports what it did.".into(),
                instructions: Some(
                    "Make the smallest change that satisfies the task, then summarize it.".into(),
                ),
                tools: Some(vec!["read".into(), "edit".into(), "shell".into()]),
                model: Some("reasoning".into()),
                effort: Some("medium".into()),
                params: Map::new(),
                tags: vec!["code".into()],
                metadata: Map::new(),
                harnesses: Default::default(),
            },
            AgentTemplate {
                id: "demo-reviewer".into(),
                name: Some("Reviewer".into()),
                description: "Reviews a diff and reports what would break.".into(),
                instructions: None,
                tools: Some(vec!["read".into()]),
                model: Some("reasoning".into()),
                effort: None,
                params: Map::new(),
                tags: vec!["review".into()],
                metadata: Map::new(),
                // Restricted to codex: a `harnesses` block is an allowlist as
                // well as an override set.
                harnesses: [(
                    "codex".to_string(),
                    AgentTemplateHarnessOverride {
                        model: Some("reasoning".into()),
                        effort: Some("high".into()),
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
            },
        ],
    }
}

/// The stand-in roster: one agent per demo workspace, so the placement walk has
/// something to resolve at every level of the chain.
pub fn demo_agents() -> Vec<AgentDescriptor> {
    vec![
        AgentDescriptor {
            id: "demo-dev-1".into(),
            name: "dev-1".into(),
            description: "Implements scoped changes in the medulla repo.".into(),
            availability: "online".into(),
            workspace_id: Some("demo-ws-medulla".into()),
            host_id: None,
            template_id: Some("demo-implementer".into()),
            tags: vec!["code".into()],
            metadata: json!({ "harness": "claude-code" })
                .as_object()
                .cloned()
                .unwrap_or_default(),
        },
        AgentDescriptor {
            id: "demo-review-1".into(),
            name: "review-1".into(),
            description: "Reviews diffs on the site repo.".into(),
            availability: "offline".into(),
            workspace_id: Some("demo-ws-site".into()),
            host_id: None,
            template_id: Some("demo-reviewer".into()),
            tags: vec!["review".into()],
            metadata: Map::new(),
        },
    ]
}

/// One gibibyte, for readable resource literals.
const GB: u64 = 1024 * 1024 * 1024;
