//! Unit tests for agent declarations: the derivations the roster depends on,
//! the migration seeding rule, and the config round-trip of the shape itself.

use super::declaration::*;

/// Capacity is what the orchestrator schedules against, so it may never exceed
/// what this build can actually provision. `Worktree` parses (a config from a
/// later version must not fail the whole load) but nothing carves a per-session
/// worktree yet, so advertising its eventual parallelism would put four
/// concurrent sessions in one checkout — the collision the strategy exists to
/// prevent.
#[test]
fn no_strategy_advertises_more_capacity_than_this_build_provisions() {
    assert_eq!(WorkspaceStrategy::Checkout.max_sessions(), 1);
    assert_eq!(
        WorkspaceStrategy::Worktree.max_sessions(),
        1,
        "worktree provisioning is unimplemented, so its capacity stays serial"
    );
    assert_eq!(
        AgentDeclaration::new("a", "this-device", "claude", "/repo").max_sessions(),
        1,
        "the v1 default is serial"
    );

    let mut worktree = AgentDeclaration::new("b", "this-device", "claude", "/repo");
    worktree.strategy = WorkspaceStrategy::Worktree;
    assert_eq!(
        worktree.max_sessions(),
        1,
        "a hand-written worktree declaration is still scheduled serially"
    );
}

#[test]
fn only_checkout_is_selectable_in_v1() {
    assert!(WorkspaceStrategy::Checkout.selectable());
    assert!(
        !WorkspaceStrategy::Worktree.selectable(),
        "worktree provisioning is not implemented"
    );
    assert_eq!(SELECTABLE_STRATEGIES, &[WorkspaceStrategy::Checkout]);
}

/// A `worktree` strategy someone hand-wrote must still parse — the variant
/// exists so a config from a later version does not fail the whole load.
#[test]
fn a_declaration_round_trips_through_its_serialized_form() {
    let mut declaration = AgentDeclaration::new("api-codex", "this-device", "codex", "/srv/api");
    declaration.name = Some("API".to_string());
    declaration.roles = vec!["reviewer".to_string()];
    declaration.strategy = WorkspaceStrategy::Worktree;
    declaration.workspace.kind = "worktree".to_string();

    let json = serde_json::to_string(&declaration).unwrap();
    assert!(json.contains("\"agentId\":\"api-codex\""), "{json}");
    assert!(
        json.contains("\"workspace\":{\"path\":\"/srv/api\",\"type\":\"worktree\"}"),
        "the workspace type is `type` on the wire: {json}"
    );
    assert!(json.contains("\"strategy\":\"worktree\""), "{json}");

    let parsed: AgentDeclaration = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, declaration);
}

/// The minimum a hand-written declaration can say. Everything absent takes the
/// v1 default rather than failing the load.
#[test]
fn a_minimal_declaration_defaults_to_a_checkout() {
    let parsed: AgentDeclaration =
        serde_json::from_str(r#"{"agentId":"a","harness":"claude","workspace":{"path":"/w"}}"#)
            .unwrap();
    assert_eq!(parsed.strategy, WorkspaceStrategy::Checkout);
    assert_eq!(parsed.workspace.kind, WORKSPACE_TYPE_CHECKOUT);
    assert!(parsed.roles.is_empty());
    assert_eq!(parsed.name, None);
    assert_eq!(parsed.max_sessions(), 1);
}

#[test]
fn a_blank_workspace_path_is_no_placement_at_all() {
    assert_eq!(WorkspaceRef::checkout("/repo").path(), Some("/repo"));
    assert_eq!(WorkspaceRef::checkout("   ").path(), None);
    assert_eq!(WorkspaceRef::default().path(), None);
}

/// The migration: the pre-declaration roster entry keeps its exact id, so
/// nothing that remembered it stops resolving.
#[test]
fn seeding_gives_the_default_harness_the_hosts_own_id() {
    let seeded = seed_declarations("this-device", "/repo", &["claude"], "claude");

    assert_eq!(seeded.len(), 1);
    assert_eq!(seeded[0].agent_id, "this-device");
    assert_eq!(seeded[0].host_id, "this-device");
    assert_eq!(seeded[0].harness, "claude");
    assert_eq!(seeded[0].workspace.path, "/repo");
    assert_eq!(seeded[0].workspace.kind, WORKSPACE_TYPE_CHECKOUT);
    assert_eq!(seeded[0].strategy, WorkspaceStrategy::Checkout);
    assert!(seeded[0].roles.is_empty());
    assert_eq!(
        seeded[0].name, None,
        "a seed names nothing the operator did not"
    );
}

/// The content change the model exists for: two CLIs on one machine are two
/// agents, not one entry describing both in prose.
#[test]
fn seeding_declares_one_agent_per_detected_harness() {
    let seeded = seed_declarations("this-device", "/repo", &["claude", "codex"], "claude");

    let ids: Vec<&str> = seeded.iter().map(|d| d.agent_id.as_str()).collect();
    assert_eq!(ids, ["this-device", "this-device-codex"]);
    assert!(seeded.iter().all(|d| d.workspace.path == "/repo"));
    assert!(seeded.iter().all(|d| d.on_host("this-device")));
}

#[test]
fn seeding_leads_with_the_default_even_when_detection_lists_it_late() {
    let seeded = seed_declarations("box", "/repo", &["claude", "codex"], "codex");

    let ids: Vec<&str> = seeded.iter().map(|d| d.agent_id.as_str()).collect();
    assert_eq!(ids, ["box", "box-claude"]);
    assert_eq!(seeded[0].harness, "codex");
}

/// A daemon that reported nothing still serves its default, so the seed must
/// not be empty — an empty seed is the blank roster this rule exists to prevent.
#[test]
fn seeding_covers_a_default_no_detection_reported() {
    let seeded = seed_declarations("box", "/repo", &[], "claude");
    assert_eq!(seeded.len(), 1);
    assert_eq!(seeded[0].agent_id, "box");
    assert_eq!(seeded[0].harness, "claude");

    let deduped = seed_declarations("box", "/repo", &["claude", "claude", "  "], "claude");
    assert_eq!(deduped.len(), 1, "a harness listed twice is one agent");
}

#[test]
fn a_suggested_id_reads_as_the_folder_and_its_harness() {
    assert_eq!(
        suggest_agent_id("/srv/medulla", "claude", &[]),
        "medulla-claude"
    );
    assert_eq!(
        suggest_agent_id("/srv/My Repo", "codex", &[]),
        "my-repo-codex"
    );
    assert_eq!(suggest_agent_id("", "", &[]), "agent");
}

/// Two checkouts of one repository share a folder name by construction, and two
/// agents sharing an id would route one's work to the other.
#[test]
fn a_suggested_id_never_collides_with_one_already_declared() {
    let taken = vec!["medulla-claude".to_string(), "medulla-claude-2".to_string()];
    assert_eq!(
        suggest_agent_id("/srv/medulla", "claude", &taken),
        "medulla-claude-3"
    );
}
