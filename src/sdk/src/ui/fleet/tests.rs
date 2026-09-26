//! Unit tests for the fleet view-model: projecting the locally registered peers
//! onto the containment chain, merging that with what the runtime declares, and
//! the env-gated stand-in fleet.

use crate::runtime::fleet::{CapacitySnapshot, HostDescriptor, HostResources};

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
            ..Default::default()
        }),
        metadata: Default::default(),
    }
}

// --- local registry → capacity ---------------------------------------------

use crate::protocol::{BudgetSource, BudgetWindow, HarnessProvider, HarnessReadiness};
use crate::runtime::WorkerInfo;

/// A registered peer with the capacity facts the projection reads.
fn peer(id: &str) -> WorkerInfo {
    WorkerInfo {
        id: id.into(),
        address: format!("{id}.example:9000"),
        handle: Some(format!("@{id}")),
        label: Some(format!("{id} label")),
        harness: Some("codex".into()),
        workspace: None,
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
    p.budgets = vec![crate::protocol::HarnessBudget {
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
fn the_demo_fleet_is_a_walkable_chain() {
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
    }
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
