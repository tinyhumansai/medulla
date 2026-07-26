//! Unit tests for the fleet view-model: the tree walk's shape and ordering, the
//! dangling-parent and unplaced-agent fallbacks, budget selection, and the
//! detail pane's placement resolution.

use serde_json::json;

use crate::runtime::fleet::{
    AgentTemplate, CapacitySnapshot, HarnessBudget, HarnessDescriptor, HostDescriptor,
    HostResources, WorkspaceDescriptor,
};
use crate::runtime::AgentDescriptor;

use super::{fleet_detail, fleet_rows, FleetNodeKind};

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
fn templates_render_after_the_chain_with_their_allowed_harnesses() {
    let (mut capacity, roster) = chain();
    capacity.templates.push(AgentTemplate {
        id: "reviewer".into(),
        name: Some("Reviewer".into()),
        description: "Reviews a diff.".into(),
        instructions: None,
        tools: None,
        model: Some("reasoning".into()),
        effort: None,
        params: Default::default(),
        tags: vec!["review".into()],
        metadata: Default::default(),
        harnesses: [("claude-code".to_string(), Default::default())]
            .into_iter()
            .collect(),
    });
    let rows = fleet_rows(&capacity, &roster);
    let template = rows.last().unwrap();
    assert_eq!(template.kind, FleetNodeKind::Template);
    assert_eq!(template.label, "Reviewer");
    assert!(template.detail.contains("on claude-code"));
    assert_eq!(rows[rows.len() - 2].kind, FleetNodeKind::Section);
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
