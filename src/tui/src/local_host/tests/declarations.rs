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

/// Start the primary host with `declared`, returning what it advertises and the
/// declarations it dropped on the way.
fn hosted(declared: &[AgentDeclaration]) -> (Vec<WorkerSpec>, Vec<String>) {
    let network = LocalBridgeNetwork::new();
    let config = HostSection::default();
    let env = env_with_only_claude();
    let options = options_from_config(
        &config,
        &env,
        None,
        None,
        None,
        &crate::local_host::LaunchPolicy {
            attribution: true,
            ..Default::default()
        },
    )
    .expect("valid config");

    let (host, problems) = start(
        &config,
        &HashMap::new(),
        &network,
        options,
        PtyManager::new(),
        declared,
    )
    .expect("a declaration never stops the host from starting")
    .expect("hosting is on by default");

    (host.specs().to_vec(), problems)
}

/// Start the primary host with `declared` and return what it would advertise.
fn advertised(declared: &[AgentDeclaration]) -> Vec<WorkerSpec> {
    let (specs, problems) = hosted(declared);
    assert!(
        problems.is_empty(),
        "every agent is declared where its host works: {problems:?}"
    );
    specs
}

/// The directory the primary host actually runs tasks in — the one every agent
/// declared on it has to name, and the one they are all advertised at.
fn host_workspace() -> String {
    medulla::daemon::embedded::resolve_workspace("")
}

/// An agent on this device, declared where this device works.
fn declaration(id: &str, harness: &str) -> AgentDeclaration {
    AgentDeclaration::new(id, "this-device", harness, host_workspace())
}

/// The collapse this model removes: a machine used to be one roster entry
/// whatever it ran, with its other CLIs surviving only as prose in the
/// description — and prose is not something a dispatch can target.
#[tokio::test]
async fn a_host_advertises_one_entry_per_declared_agent() {
    let specs = advertised(&[
        declaration("api-claude", "claude"),
        declaration("api-codex", "codex"),
        declaration("api-reviewer", "claude"),
    ]);

    let ids: Vec<&str> = specs.iter().map(|spec| spec.id.as_str()).collect();
    assert_eq!(ids, ["api-claude", "api-codex", "api-reviewer"]);
    // One machine, one bus address: the id is what tells the agents apart.
    assert!(specs.iter().all(|spec| spec.address == "this-device"));
    assert!(specs.iter().all(|spec| spec.host_id == "this-device"));
    let harnesses: Vec<&str> = specs.iter().map(|spec| spec.harness.as_str()).collect();
    assert_eq!(harnesses, ["claude", "codex", "claude"]);
    // Every one of them is advertised at the directory the host runs tasks in,
    // because that is where every one of them runs.
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
    let host = host_workspace();
    assert_eq!(workspaces, [host.as_str(), host.as_str(), host.as_str()]);
}

/// A declaration that names no directory is not a *wrong* directory: it takes
/// its host's, which is what it would have done anyway.
#[tokio::test]
async fn an_agent_that_declares_no_workspace_takes_its_hosts() {
    let mut blank = declaration("api-claude", "claude");
    blank.workspace.path = "   ".to_string();

    let specs = advertised(&[blank]);

    assert_eq!(
        specs[0].workspace.as_ref().expect("a placed agent").path,
        host_workspace()
    );
}

/// Declarations for another machine are that machine's business — advertising
/// them here would tell the orchestrator this host can serve work it cannot.
#[tokio::test]
async fn a_host_advertises_only_the_agents_declared_on_it() {
    let specs = advertised(&[
        declaration("mine", "claude"),
        // Another machine's agent, in another machine's directory. Not checked
        // against this host's workspace either — it is not this host's business.
        AgentDeclaration::new("theirs", "mac-studio", "codex", "/srv/api"),
    ]);

    let ids: Vec<&str> = specs.iter().map(|spec| spec.id.as_str()).collect();
    assert_eq!(ids, ["mine"]);
}

/// A declaration naming a directory this host does not run in costs *that
/// declaration*, not hosting on this device.
///
/// The regression: it used to fail the whole host, and a device with no host
/// has no sessions to start and shows up offline in the fleet — so one mistyped
/// directory read as "my laptop is not part of this any more", with the reason
/// visible only in the log.
#[tokio::test]
async fn a_misplaced_declaration_is_dropped_and_the_host_still_starts() {
    let (specs, problems) = hosted(&[
        declaration("here", "claude"),
        AgentDeclaration::new("elsewhere", "this-device", "claude", "/srv/api"),
    ]);

    let ids: Vec<&str> = specs.iter().map(|spec| spec.id.as_str()).collect();
    assert_eq!(ids, ["here"], "the misplaced agent is not advertised");
    assert_eq!(problems.len(), 1, "and the operator is told which one");
    assert!(
        problems[0].contains("elsewhere") && problems[0].contains("/srv/api"),
        "the report names the agent and the directory: {}",
        problems[0]
    );
    assert!(
        !problems[0].contains('\n'),
        "startup status renders this warning on one line: {}",
        problems[0]
    );
}

/// Every declaration misplaced is still a hosting device: it advertises what it
/// detected, exactly as an install that declared nothing does. Left to fall
/// through, the host would advertise an empty roster instead — unroutable, and
/// indistinguishable on screen from not hosting at all.
#[tokio::test]
async fn a_host_whose_every_declaration_is_misplaced_falls_back_to_the_seed() {
    let (specs, problems) = hosted(&[AgentDeclaration::new(
        "elsewhere",
        "this-device",
        "claude",
        "/srv/api",
    )]);

    assert_eq!(problems.len(), 1);
    assert_eq!(specs.len(), 1, "the detection seed stands in");
    assert_eq!(specs[0].id, "this-device");
    assert_eq!(specs[0].harness, "claude");
}

/// A rejected declaration must not be replaced by a fallback with the same ID:
/// routing for the rejected placement would otherwise reach this host.
#[tokio::test]
async fn a_fallback_does_not_reuse_a_misplaced_declarations_agent_id() {
    let (specs, problems) = hosted(&[AgentDeclaration::new(
        "this-device",
        "this-device",
        "claude",
        "/srv/api",
    )]);

    assert_eq!(problems.len(), 1);
    assert_eq!(specs.len(), 1, "the detected provider still has a fallback");
    assert_ne!(specs[0].id, "this-device");
    assert_eq!(specs[0].id, "this-device-fallback-1");
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
    let single = advertised(&[declaration("solo", "claude")]);
    assert_eq!(single[0].name, "this device");

    let several = advertised(&[declaration("a", "claude"), declaration("b", "codex")]);
    assert_eq!(several[0].name, "this device · claude");
    assert_eq!(several[1].name, "this device · codex");

    let mut named = declaration("a", "claude");
    named.name = Some("API reviewer".to_string());
    let chosen = advertised(&[named, declaration("b", "codex")]);
    assert_eq!(chosen[0].name, "API reviewer", "an operator's name wins");
}

/// Two agents of the same harness is an ordinary declaration — a reviewer and a
/// writer on one machine — and suffixing both with "claude" names neither. The
/// agent id is what a dispatch targets, so it is what the rows fall back to.
#[tokio::test]
async fn siblings_sharing_a_harness_are_named_by_their_agent_id() {
    let specs = advertised(&[
        declaration("api-reviewer", "claude"),
        declaration("api-writer", "claude"),
        declaration("api-codex", "codex"),
    ]);

    let names: Vec<&str> = specs.iter().map(|spec| spec.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "this device · api-reviewer",
            "this device · api-writer",
            // The lone codex keeps the harness suffix: nothing to disambiguate
            // it from, and the harness is the more readable of the two.
            "this device · codex",
        ]
    );
    let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len(), "no two rows share a label");
}

/// Capacity rides the advert from the declaration's strategy, which is the one
/// thing that decides how many sessions an agent may run at once.
#[tokio::test]
async fn the_declared_strategy_reaches_the_roster_entry() {
    let mut declared = declaration("api-claude", "claude");
    declared.strategy = WorkspaceStrategy::Checkout;

    let specs = advertised(&[declared]);

    assert_eq!(
        specs[0].max_sessions, 1,
        "capacity is derived from the strategy, never configured"
    );
}
