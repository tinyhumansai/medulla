//! Unit tests for the fleet view-model: the tree walk's shape and ordering, the
//! dangling-parent and unplaced-agent fallbacks, budget selection, and the
//! detail pane's placement resolution.

use serde_json::json;

use crate::runtime::fleet::{
    AgentTemplate, CapacitySnapshot, HarnessBudget, HarnessDescriptor, HostDescriptor,
    HostResources, WorkspaceDescriptor,
};
use crate::runtime::AgentDescriptor;

use super::{fleet_detail, fleet_rows, places_allowing, template_rows, FleetNodeKind};

fn host(id: &str) -> HostDescriptor {
    HostDescriptor {
        id: id.into(),
        name: format!("{id}-box"),
        availability: "online".into(),
        address: None,
        resources: Some(HostResources {
            cpu_cores: Some(8.0),
            total_memory_bytes: Some(32 << 30),
            available_memory_bytes: Some(12 << 30),
            disk_free_bytes: None,
        }),
        metadata: Default::default(),
    }
}

fn harness(id: &str, host_id: &str, kind: &str) -> HarnessDescriptor {
    HarnessDescriptor {
        id: id.into(),
        host_id: host_id.into(),
        kind: kind.into(),
        availability: "online".into(),
        ready: true,
        ready_reason: None,
        providers: Vec::new(),
        template_ids: Vec::new(),
        budgets: Vec::new(),
        metadata: Default::default(),
    }
}

fn workspace(id: &str, harness_id: &str, path: &str) -> WorkspaceDescriptor {
    WorkspaceDescriptor {
        id: id.into(),
        name: path.into(),
        path: path.into(),
        harness_id: harness_id.into(),
        profile: None,
        project: None,
        template_ids: Vec::new(),
        metadata: Default::default(),
    }
}

fn agent(id: &str) -> AgentDescriptor {
    AgentDescriptor {
        id: id.into(),
        name: id.into(),
        description: String::new(),
        availability: "online".into(),
        workspace_id: None,
        host_id: None,
        template_id: None,
        tags: Vec::new(),
        metadata: Default::default(),
    }
}

/// A template optionally restricted to the named harness kinds.
fn template(id: &str, harnesses: &[&str]) -> AgentTemplate {
    AgentTemplate {
        id: id.into(),
        name: Some(format!("{id}-name")),
        description: "Does a thing.".into(),
        instructions: None,
        tools: None,
        model: Some("reasoning".into()),
        effort: None,
        params: Default::default(),
        tags: Vec::new(),
        metadata: Default::default(),
        harnesses: harnesses
            .iter()
            .map(|kind| (kind.to_string(), Default::default()))
            .collect(),
    }
}

fn budget(remaining: u64, limit: u64) -> HarnessBudget {
    HarnessBudget {
        provider: "anthropic".into(),
        window: "5h".into(),
        seat: None,
        limit_tokens: Some(limit),
        used_tokens: None,
        remaining_tokens: Some(remaining),
        cooldown_until: None,
        source: "provider_reported".into(),
    }
}

/// A full chain: one host, one harness, one workspace, one agent in it.
fn chain() -> (CapacitySnapshot, Vec<AgentDescriptor>) {
    let capacity = CapacitySnapshot {
        hosts: vec![host("h1")],
        harnesses: vec![harness("hn1", "h1", "claude-code")],
        workspaces: vec![workspace("ws1", "hn1", "/srv/repo")],
        templates: Vec::new(),
    };
    let mut a = agent("a1");
    a.workspace_id = Some("ws1".into());
    (capacity, vec![a])
}

#[test]
fn nothing_declared_renders_no_rows() {
    assert!(fleet_rows(&CapacitySnapshot::default(), &[]).is_empty());
}

#[test]
fn walks_the_chain_top_down_with_increasing_depth() {
    let (capacity, roster) = chain();
    let rows = fleet_rows(&capacity, &roster);
    let shape: Vec<(FleetNodeKind, usize)> = rows.iter().map(|r| (r.kind, r.depth)).collect();
    assert_eq!(
        shape,
        vec![
            (FleetNodeKind::Host, 0),
            (FleetNodeKind::Harness, 1),
            (FleetNodeKind::Workspace, 2),
            (FleetNodeKind::Agent, 3),
        ]
    );
    assert!(rows[0].detail.contains("1 agent"));
    assert!(rows[0].detail.contains("8 cores"));
}

#[test]
fn an_agent_pinned_to_a_host_hangs_off_it_directly() {
    let mut capacity = CapacitySnapshot::default();
    capacity.hosts.push(host("h1"));
    let mut local = agent("local");
    local.host_id = Some("h1".into());
    let rows = fleet_rows(&capacity, &[local]);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].kind, FleetNodeKind::Agent);
    assert_eq!(rows[1].depth, 1);
}

#[test]
fn an_agent_with_no_resolvable_placement_lands_in_the_unplaced_group() {
    let rows = fleet_rows(&CapacitySnapshot::default(), &[agent("stray")]);
    assert_eq!(rows[0].kind, FleetNodeKind::Section);
    assert!(rows[0].label.contains("unplaced"));
    assert!(!rows[0].kind.selectable());
    assert_eq!(rows[1].key, "agent:stray");
}

#[test]
fn a_harness_whose_host_is_undeclared_still_renders() {
    let capacity = CapacitySnapshot {
        harnesses: vec![harness("hn1", "ghost", "codex")],
        ..Default::default()
    };
    let rows = fleet_rows(&capacity, &[]);
    assert_eq!(rows[0].label, "ghost");
    assert!(rows[0].degraded, "a dangling parent reads as degraded");
    assert_eq!(rows[1].kind, FleetNodeKind::Harness);
}

#[test]
fn a_harness_row_shows_its_tightest_budget_and_offline_reads_degraded() {
    let mut capacity = CapacitySnapshot {
        hosts: vec![host("h1")],
        harnesses: vec![harness("hn1", "h1", "codex")],
        ..Default::default()
    };
    capacity.harnesses[0].budgets = vec![budget(900_000, 1_000_000), budget(50_000, 1_000_000)];
    capacity.harnesses[0].ready = false;
    capacity.harnesses[0].ready_reason = Some("cli not installed".into());

    let rows = fleet_rows(&capacity, &[]);
    let harness_row = &rows[1];
    assert!(harness_row.detail.contains("50k/1.0M left"));
    assert!(harness_row.detail.contains("cli not installed"));
    assert!(harness_row.degraded);
}

#[test]
fn the_chain_holds_no_templates_of_its_own() {
    let (mut capacity, roster) = chain();
    capacity
        .templates
        .push(template("reviewer", &["claude-code"]));
    // A template constrains the chain rather than sitting in it, so the tree is
    // byte-identical whether or not a catalog is declared.
    assert_eq!(
        fleet_rows(&capacity, &roster),
        fleet_rows(
            &CapacitySnapshot {
                templates: Vec::new(),
                ..capacity.clone()
            },
            &roster
        )
    );
}

#[test]
fn the_template_catalog_reports_where_it_may_run_and_what_it_provisioned() {
    let (mut capacity, mut roster) = chain();
    capacity
        .templates
        .push(template("reviewer", &["claude-code"]));
    roster[0].template_id = Some("reviewer".into());

    let rows = template_rows(&capacity, &roster);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, FleetNodeKind::Template);
    assert_eq!(rows[0].label, "reviewer-name");
    assert!(rows[0].detail.contains("on claude-code"));
    assert!(rows[0].detail.contains("1 place"));
    assert!(rows[0].detail.contains("1 agent"));
    assert!(!rows[0].degraded);
}

#[test]
fn a_template_no_place_admits_reads_degraded() {
    let (mut capacity, roster) = chain();
    // The only workspace is exposed by a claude-code harness, so a codex-only
    // template has nowhere to run.
    capacity.templates.push(template("codex-only", &["codex"]));
    let rows = template_rows(&capacity, &roster);
    assert!(rows[0].degraded);
    assert!(rows[0].detail.contains("nowhere allows it"));
}

#[test]
fn an_allowlist_only_ever_subtracts() {
    let (mut capacity, _) = chain();
    let unrestricted = template("any", &[]);
    capacity.templates.push(unrestricted.clone());
    // No allowlist anywhere: the whole catalog is admitted.
    assert_eq!(places_allowing(&unrestricted, &capacity), 1);

    // A workspace allowlist that names something else excludes it.
    capacity.workspaces[0].template_ids = vec!["other".into()];
    assert_eq!(places_allowing(&unrestricted, &capacity), 0);

    // Naming it back admits it again.
    capacity.workspaces[0].template_ids = vec!["any".into()];
    assert_eq!(places_allowing(&unrestricted, &capacity), 1);

    // A harness allowlist subtracts independently of the workspace's.
    capacity.harnesses[0].template_ids = vec!["other".into()];
    assert_eq!(places_allowing(&unrestricted, &capacity), 0);
}

#[test]
fn agent_detail_resolves_the_whole_chain_upward() {
    let (capacity, roster) = chain();
    let lines: Vec<String> = fleet_detail(&capacity, &roster, "agent:a1", 60)
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert!(lines.iter().any(|l| l == "host: h1-box"));
    assert!(lines.iter().any(|l| l == "harness: claude-code"));
    assert!(lines.iter().any(|l| l == "workspace: /srv/repo"));
}

#[test]
fn agent_detail_names_a_local_agent_as_having_no_workspace() {
    let lines: Vec<String> = fleet_detail(
        &CapacitySnapshot::default(),
        &[agent("solo")],
        "agent:solo",
        60,
    )
    .into_iter()
    .map(|l| l.text)
    .collect();
    assert!(lines.iter().any(|l| l.contains("local agent")));
}

#[test]
fn harness_detail_lists_every_budget_window() {
    let mut capacity = CapacitySnapshot {
        hosts: vec![host("h1")],
        harnesses: vec![harness("hn1", "h1", "codex")],
        ..Default::default()
    };
    capacity.harnesses[0].budgets = vec![budget(900_000, 1_000_000), budget(50_000, 1_000_000)];
    let lines: Vec<String> = fleet_detail(&capacity, &[], "harness:hn1", 60)
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert!(lines.iter().any(|l| l.contains("900k/1.0M left")));
    assert!(lines.iter().any(|l| l.contains("50k/1.0M left")));
}

#[test]
fn a_stale_or_malformed_selection_renders_nothing() {
    let (capacity, roster) = chain();
    assert!(fleet_detail(&capacity, &roster, "host:gone", 60).is_empty());
    assert!(fleet_detail(&capacity, &roster, "bogus", 60).is_empty());
}

#[test]
fn workspace_detail_shows_its_medulla_md_profile() {
    let (mut capacity, roster) = chain();
    capacity.workspaces[0].profile = Some(crate::runtime::fleet::WorkspaceProfile {
        instructions: "Prefer small commits.".into(),
        harnesses: vec!["claude-code".into()],
        models: Default::default(),
        metadata: Default::default(),
    });
    capacity.workspaces[0].metadata = json!({ "branch": "main" }).as_object().cloned().unwrap();
    let lines: Vec<String> = fleet_detail(&capacity, &roster, "workspace:ws1", 60)
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert!(lines.iter().any(|l| l.contains("Prefer small commits.")));
    assert!(lines.iter().any(|l| l.contains("branch: main")));
}

// --- local registry → capacity ---------------------------------------------

use crate::runtime::WorkerInfo;
use crate::tinyplace::{BudgetSource, BudgetWindow, HarnessProvider, HarnessReadiness};

/// A registered peer with the capacity facts the Hosts page shows.
fn peer(id: &str) -> WorkerInfo {
    WorkerInfo {
        id: id.into(),
        address: format!("{id}.example:9000"),
        handle: Some(format!("@{id}")),
        label: Some(format!("{id} label")),
        harness: Some("codex".into()),
        peer_id: None,
        cpu_cores: Some(8),
        memory_total_bytes: Some(32 << 30),
        memory_available_bytes: Some(18 << 30),
        ip_address: Some("10.0.0.9".into()),
        selected: false,
        budgets: Vec::new(),
        readiness: Vec::new(),
    }
}

#[test]
fn a_registered_peer_becomes_a_host_with_its_advertised_harness() {
    let capacity = super::registry_capacity(&[peer("w1")]);
    assert_eq!(capacity.hosts.len(), 1);
    assert_eq!(capacity.hosts[0].name, "w1 label");
    assert_eq!(capacity.hosts[0].address.as_deref(), Some("10.0.0.9"));
    assert_eq!(
        capacity.hosts[0].resources.as_ref().unwrap().cpu_cores,
        Some(8.0)
    );
    // Reachability is not liveness: the registry must not claim "online".
    assert!(capacity.hosts[0].availability.is_empty());
    assert_eq!(capacity.harnesses.len(), 1);
    assert_eq!(capacity.harnesses[0].kind, "codex");
    assert_eq!(capacity.harnesses[0].host_id, capacity.hosts[0].id);
    assert!(capacity.harnesses[0].ready, "an unprobed runtime is usable");
}

#[test]
fn probed_readiness_and_budgets_split_into_one_harness_per_provider() {
    let mut p = peer("w1");
    p.readiness = vec![
        HarnessReadiness {
            provider: HarnessProvider::Claude,
            ready: true,
            reason: None,
        },
        HarnessReadiness {
            provider: HarnessProvider::Codex,
            ready: false,
            reason: Some("not authenticated".into()),
        },
    ];
    p.budgets = vec![crate::tinyplace::HarnessBudget {
        provider: HarnessProvider::Claude,
        seat: Some("seat-1".into()),
        window: BudgetWindow::FiveHour,
        limit_tokens: Some(1_000_000),
        used_tokens: Some(250_000),
        remaining_tokens: Some(750_000),
        cooldown_until: None,
        source: BudgetSource::ProviderReported,
    }];

    let capacity = super::registry_capacity(&[p]);
    assert_eq!(capacity.harnesses.len(), 2);
    let claude = &capacity.harnesses[0];
    assert_eq!(claude.kind, "claude");
    assert_eq!(claude.budgets[0].window, "5h");
    assert_eq!(claude.budgets[0].remaining(), Some(750_000));
    let codex = &capacity.harnesses[1];
    assert!(!codex.ready);
    assert_eq!(codex.ready_reason.as_deref(), Some("not authenticated"));
    assert!(codex.budgets.is_empty(), "budgets follow their provider");
}

#[test]
fn merging_never_lists_one_machine_twice() {
    let declared = CapacitySnapshot {
        hosts: vec![HostDescriptor {
            address: Some("10.0.0.9".into()),
            ..host("declared")
        }],
        ..Default::default()
    };
    // Same address under a different id: still one machine.
    let merged = super::merge_capacity(&declared, &super::registry_capacity(&[peer("w1")]));
    assert_eq!(merged.hosts.len(), 1);
    assert_eq!(merged.hosts[0].id, "declared");
    assert!(merged.harnesses.is_empty(), "its harnesses are dropped too");

    // A machine nothing declared is added, with its harnesses.
    let mut elsewhere = peer("w2");
    elsewhere.ip_address = Some("10.0.0.10".into());
    let merged = super::merge_capacity(&declared, &super::registry_capacity(&[elsewhere]));
    assert_eq!(merged.hosts.len(), 2);
    assert_eq!(merged.harnesses.len(), 1);
}

// --- the env-gated stand-in fleet -------------------------------------------

#[test]
fn the_demo_fleet_is_a_walkable_chain_with_a_restricted_template() {
    let capacity = crate::runtime::demo_capacity();
    let agents = crate::runtime::demo_agents();

    // Every demo agent resolves all the way up to a host.
    for agent in &agents {
        let placement = capacity.placement(agent);
        assert!(
            placement.workspace.is_some(),
            "{} has a workspace",
            agent.id
        );
        assert!(placement.harness.is_some(), "{} has a harness", agent.id);
        assert!(placement.host.is_some(), "{} has a host", agent.id);
        assert!(placement.template.is_some(), "{} has a template", agent.id);
    }

    // The rows the surfaces actually render are non-empty and well-formed.
    let rows = fleet_rows(&capacity, &agents);
    assert!(rows.iter().any(|r| r.kind == FleetNodeKind::Host));
    assert!(rows.iter().any(|r| r.kind == FleetNodeKind::Workspace));
    assert!(
        !rows.iter().any(|r| r.label.contains("unplaced")),
        "the demo chain places every demo agent"
    );

    // The reviewer is codex-only and the site workspace allowlists it, so it may
    // run in exactly one place — which is what makes the allowlist visible.
    let reviewer = capacity.template("demo-reviewer").expect("reviewer");
    assert_eq!(places_allowing(reviewer, &capacity), 1);
}

#[test]
fn the_demo_flag_is_opt_in_and_ignores_negative_spellings() {
    use crate::runtime::demo_requested_from;
    assert!(!demo_requested_from(None), "unset means off");
    for off in ["", "  ", "0", "false", "FALSE", "no", "off"] {
        assert!(!demo_requested_from(Some(off)), "{off:?} must read as off");
    }
    for on in ["1", "true", "yes", "please"] {
        assert!(demo_requested_from(Some(on)), "{on:?} must read as on");
    }
}
