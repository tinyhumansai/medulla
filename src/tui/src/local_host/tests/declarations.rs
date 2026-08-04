//! What a host advertises: one roster entry per declared agent, and the
//! migration seed that stands in for an install that declared none.

use std::collections::HashMap;

use medulla::bridge::LocalBridgeNetwork;
use medulla::config::HostSection;
use medulla::hub::WorkerSpec;
use medulla::runtime::{AgentDeclaration, WorkspaceStrategy};
use medulla_tui::worker::pty::PtyManager;

use crate::local_host::{options_from_config, start};

use super::env_with_only_claude;

/// Start the primary host with `declared` and return what it would advertise.
fn advertised(declared: &[AgentDeclaration]) -> Vec<WorkerSpec> {
    let network = LocalBridgeNetwork::new();
    let config = HostSection::default();
    let env = env_with_only_claude();
    let options = options_from_config(&config, &env, None, None, None, true).expect("valid config");

    let host = start(
        &config,
        &HashMap::new(),
        &network,
        options,
        PtyManager::new(),
        declared,
    )
    .unwrap()
    .expect("hosting is on by default");

    host.specs().to_vec()
}

fn declaration(id: &str, harness: &str, workspace: &str) -> AgentDeclaration {
    AgentDeclaration::new(id, "this-device", harness, workspace)
}

/// The collapse this model removes: a machine used to be one roster entry
/// whatever it ran, with its other CLIs surviving only as prose in the
/// description — and prose is not something a dispatch can target.
#[tokio::test]
async fn a_host_advertises_one_entry_per_declared_agent() {
    let specs = advertised(&[
        declaration("api-claude", "claude", "/srv/api"),
        declaration("api-codex", "codex", "/srv/api"),
        declaration("web-claude", "claude", "/srv/web"),
    ]);

    let ids: Vec<&str> = specs.iter().map(|spec| spec.id.as_str()).collect();
    assert_eq!(ids, ["api-claude", "api-codex", "web-claude"]);
    // One machine, one bus address: the id is what tells the agents apart.
    assert!(specs.iter().all(|spec| spec.address == "this-device"));
    assert!(specs.iter().all(|spec| spec.host_id == "this-device"));
    let harnesses: Vec<&str> = specs.iter().map(|spec| spec.harness.as_str()).collect();
    assert_eq!(harnesses, ["claude", "codex", "claude"]);
    let workspaces: Vec<&str> = specs
        .iter()
        .map(|spec| {
            spec.workspace
                .as_ref()
                .expect("a placed agent")
                .path
                .as_str()
        })
        .collect();
    assert_eq!(workspaces, ["/srv/api", "/srv/api", "/srv/web"]);
}

/// Declarations for another machine are that machine's business — advertising
/// them here would tell the orchestrator this host can serve work it cannot.
#[tokio::test]
async fn a_host_advertises_only_the_agents_declared_on_it() {
    let specs = advertised(&[
        declaration("mine", "claude", "/srv/api"),
        AgentDeclaration::new("theirs", "mac-studio", "codex", "/srv/api"),
    ]);

    let ids: Vec<&str> = specs.iter().map(|spec| spec.id.as_str()).collect();
    assert_eq!(ids, ["mine"]);
}

/// The upgrade path: an install that predates declarations must not go from one
/// worker in the roster to none.
#[tokio::test]
async fn a_host_with_no_declarations_is_seeded_from_what_it_detected() {
    let specs = advertised(&[]);

    assert_eq!(specs.len(), 1, "one detected CLI, one agent");
    assert_eq!(
        specs[0].id, "this-device",
        "the pre-declaration entry keeps its exact id"
    );
    assert_eq!(specs[0].harness, "claude");
    assert_eq!(specs[0].name, "this device");
}

/// The name is what the rail and the fleet view read. A lone agent stays "this
/// device" — the machine reads exactly as it did — and siblings say which
/// harness they are, because two rows with one name identify neither.
#[tokio::test]
async fn agent_names_disambiguate_only_when_there_is_something_to_disambiguate() {
    let single = advertised(&[declaration("solo", "claude", "/srv/api")]);
    assert_eq!(single[0].name, "this device");

    let several = advertised(&[
        declaration("a", "claude", "/srv/api"),
        declaration("b", "codex", "/srv/api"),
    ]);
    assert_eq!(several[0].name, "this device · claude");
    assert_eq!(several[1].name, "this device · codex");

    let mut named = declaration("a", "claude", "/srv/api");
    named.name = Some("API reviewer".to_string());
    let chosen = advertised(&[named, declaration("b", "codex", "/srv/api")]);
    assert_eq!(chosen[0].name, "API reviewer", "an operator's name wins");
}

/// Roles are assigned on the declaration and ride the advert as
/// `metadata.roles`. The emit path already existed; the declaration is what
/// finally gives it a source for an agent on this machine.
#[tokio::test]
async fn declared_roles_and_strategy_reach_the_roster_entry() {
    let mut declared = declaration("api-claude", "claude", "/srv/api");
    declared.roles = vec!["code-reviewer".to_string(), "test-writer".to_string()];
    declared.strategy = WorkspaceStrategy::Checkout;

    let specs = advertised(&[declared]);

    assert_eq!(specs[0].roles, ["code-reviewer", "test-writer"]);
    assert_eq!(
        specs[0].max_sessions, 1,
        "capacity is derived from the strategy, never configured"
    );
}
