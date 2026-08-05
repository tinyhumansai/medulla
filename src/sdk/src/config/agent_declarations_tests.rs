//! Unit tests for the declared-agent store: the CRUD round-trip through the
//! config file, and the queries the roster and the UI read it back with.

use std::path::PathBuf;

use super::*;
use crate::runtime::{AgentDeclaration, WorkspaceStrategy};

/// A config file in a scratch directory, plus the directory keeping it alive.
fn scratch(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join(name);
    (dir, path)
}

fn declaration_for(id: &str, harness: &str, workspace: &str) -> AgentDeclaration {
    AgentDeclaration::new(id, "this-device", harness, workspace)
}

/// The whole point of a declaration: it is still there on the next launch, in
/// the shape the roster reads.
#[test]
fn a_declared_agent_round_trips_through_the_config_file() {
    let (_dir, path) = scratch("config.toml");
    let mut declared = declaration_for("api-claude", "claude", "/srv/api");
    declared.name = Some("API".to_string());
    declared.roles = vec!["reviewer".to_string()];

    let written = declare_agent(&path, &[], declared.clone()).expect("the config is writable");
    assert_eq!(written, vec![declared.clone()]);

    let reloaded = load_agent_declarations(&path);
    assert_eq!(reloaded, vec![declared]);
    assert_eq!(reloaded[0].max_sessions(), 1, "checkout is serial");
}

/// The same round-trip through a JSON config. Both halves of the store branch
/// on the file's extension — [`load_agent_declarations`] parses anything that is
/// not `.toml` as JSON, and the writer has its own JSON merge path — so a
/// TOML-only suite covers neither, and an operator on `medulla.tui.json` would
/// be the one to find out that their declarations reload as an empty list.
#[test]
fn a_declared_agent_round_trips_through_a_json_config() {
    let (_dir, path) = scratch("medulla.tui.json");
    let mut declared = declaration_for("api-codex", "codex", "/srv/api");
    declared.name = Some("API".to_string());
    declared.roles = vec!["reviewer".to_string()];

    let written = declare_agent(&path, &[], declared.clone()).expect("the config is writable");
    assert_eq!(written, vec![declared.clone()]);

    let text = std::fs::read_to_string(&path).expect("the config was written");
    assert!(
        serde_json::from_str::<serde_json::Value>(&text).is_ok(),
        "a .json config must stay JSON on disk: {text}"
    );

    let reloaded = load_agent_declarations(&path);
    assert_eq!(reloaded, vec![declared]);
    assert_eq!(reloaded[0].max_sessions(), 1, "checkout is serial");
}

/// Declarations live beside every other section, so writing one must not cost
/// the operator their `[host]` block.
#[test]
fn declaring_an_agent_preserves_unrelated_config() {
    let (_dir, path) = scratch("config.toml");
    std::fs::write(
        &path,
        "[host]\nworkspace = \"/srv/api\"\n\n[fleet]\nhosts = [{ id = \"this-device\", name = \"this device\", availability = \"online\" }]\n",
    )
    .unwrap();

    declare_agent(&path, &[], declaration_for("a", "claude", "/srv/api")).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    let config: TuiConfig = toml::from_str(&text).unwrap();
    assert_eq!(config.host.workspace, "/srv/api");
    assert_eq!(config.fleet.hosts.len(), 1, "the declared host survived");
    assert_eq!(config.fleet.agent_declarations.len(), 1);
}

/// Editing an agent is not declaring a second one, and it must not move it in
/// the list the operator is looking at.
#[test]
fn redeclaring_an_agent_replaces_it_in_place() {
    let (_dir, path) = scratch("config.toml");
    let first = declare_agent(&path, &[], declaration_for("a", "claude", "/one")).unwrap();
    let both = declare_agent(&path, &first, declaration_for("b", "codex", "/two")).unwrap();

    let mut edited = declaration_for("a", "claude", "/moved");
    edited.roles = vec!["reviewer".to_string()];
    let after = declare_agent(&path, &both, edited).unwrap();

    assert_eq!(after.len(), 2, "an edit is not a new agent");
    assert_eq!(after[0].agent_id, "a", "and it keeps its position");
    assert_eq!(after[0].workspace.path, "/moved");
    assert_eq!(after[0].roles, ["reviewer"]);
    assert_eq!(load_agent_declarations(&path), after);
}

#[test]
fn undeclaring_an_agent_is_durable_and_leaves_the_others() {
    let (_dir, path) = scratch("config.toml");
    let one = declare_agent(&path, &[], declaration_for("a", "claude", "/one")).unwrap();
    let two = declare_agent(&path, &one, declaration_for("b", "codex", "/two")).unwrap();

    let after = undeclare_agent(&path, &two, "a").expect("declared agents can be removed");

    assert_eq!(declared_agent_ids(&after), ["b"]);
    assert_eq!(
        load_agent_declarations(&path),
        after,
        "a removal that only lived in memory would come back at the next launch"
    );
}

/// Removing the last agent must write an empty list, not skip the write — the
/// file is the state, and "I removed the last one" has to survive a restart.
#[test]
fn undeclaring_the_last_agent_writes_the_empty_list() {
    let (_dir, path) = scratch("config.toml");
    let one = declare_agent(&path, &[], declaration_for("a", "claude", "/one")).unwrap();

    let after = undeclare_agent(&path, &one, "a").unwrap();

    assert!(after.is_empty());
    assert!(load_agent_declarations(&path).is_empty());
}

#[test]
fn undeclaring_an_unknown_agent_says_so_rather_than_writing_nothing() {
    let (_dir, path) = scratch("config.toml");
    let one = declare_agent(&path, &[], declaration_for("a", "claude", "/one")).unwrap();

    let error = undeclare_agent(&path, &one, "ghost").expect_err("nothing to remove");

    assert!(error.to_string().contains("ghost"), "{error}");
    assert_eq!(
        load_agent_declarations(&path),
        one,
        "a failed removal leaves the file alone"
    );
}

#[test]
fn declarations_are_queried_by_host_and_by_id() {
    let mut declarations = vec![
        declaration_for("a", "claude", "/one"),
        AgentDeclaration::new("remote", "mac-studio", "codex", "/two"),
    ];
    declarations[0].strategy = WorkspaceStrategy::Checkout;

    let local = agent_declarations_for_host(&declarations, "this-device");
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].agent_id, "a");

    assert_eq!(
        agent_declaration(&declarations, "remote").map(|d| d.harness.as_str()),
        Some("codex")
    );
    assert!(agent_declaration(&declarations, "nobody").is_none());
    assert_eq!(declared_agent_ids(&declarations), ["a", "remote"]);
}

#[test]
fn the_pure_helpers_report_whether_they_added_or_updated() {
    let mut declarations = Vec::new();
    assert!(upsert_agent_declaration(
        &mut declarations,
        declaration_for("a", "claude", "/one")
    ));
    assert!(!upsert_agent_declaration(
        &mut declarations,
        declaration_for("a", "codex", "/one")
    ));
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].harness, "codex");

    assert!(remove_agent_declaration(&mut declarations, "a").is_some());
    assert!(remove_agent_declaration(&mut declarations, "a").is_none());
}

/// A config file that is not there yet is not an error — it is an install with
/// nothing declared, which is where every machine starts.
#[test]
fn an_absent_config_file_declares_nothing() {
    let (_dir, path) = scratch("missing.toml");
    assert!(load_agent_declarations(&path).is_empty());

    std::fs::write(&path, "this is not toml {{{").unwrap();
    assert!(load_agent_declarations(&path).is_empty());
}
