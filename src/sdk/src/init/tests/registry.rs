//! Unit tests for workspace registration: what `medulla init` writes into the
//! operator's config so the orchestrator can see and place a workspace.

use std::fs;
use std::path::PathBuf;

use crate::config::TuiConfig;
use crate::init::registry::{
    deregister_workspace, register_workspace, workspace_id, DEFAULT_HARNESS_ID,
};
use crate::runtime::{HarnessDescriptor, WorkspaceDescriptor};

/// A unique scratch directory per test.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("medulla-registry-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A workspace directory and the config file registration writes to, both
/// inside one scratch root.
fn fixture(tag: &str) -> (PathBuf, PathBuf) {
    let root = scratch(tag);
    let workspace = root.join("repo");
    fs::create_dir_all(&workspace).expect("workspace dir");
    (workspace, root.join("config.toml"))
}

/// Re-read a written config file the same way the loader would.
fn reload(path: &std::path::Path) -> TuiConfig {
    let text = fs::read_to_string(path).expect("config written");
    toml::from_str(&text).expect("config parses")
}

/// A harness the operator has already declared.
fn harness(id: &str) -> HarnessDescriptor {
    HarnessDescriptor {
        id: id.to_string(),
        host_id: "host-1".into(),
        kind: "claude-code".into(),
        availability: "online".into(),
        ready: true,
        ready_reason: None,
        providers: Vec::new(),
        template_ids: Vec::new(),
        budgets: Vec::new(),
        metadata: serde_json::Map::new(),
    }
}

#[test]
fn register_writes_both_workspace_lists() {
    let (workspace, config_path) = fixture("both");
    let reg = register_workspace(&config_path, &TuiConfig::default(), &workspace, None).unwrap();

    assert!(reg.added);
    assert_eq!(reg.name, "repo");
    assert_eq!(reg.harness_id, DEFAULT_HARNESS_ID);

    let written = reload(&config_path);
    // The placement chain the orchestrator routes onto...
    assert_eq!(written.fleet.workspaces.len(), 1);
    let entry = &written.fleet.workspaces[0];
    assert_eq!(entry.id, reg.id);
    assert_eq!(entry.name, "repo");
    assert_eq!(entry.path, reg.path);
    // ...and the roots whose MEDULLA.md rides every session mint.
    assert_eq!(written.workflow.workspaces, vec![reg.path.clone()]);
}

#[test]
fn registered_path_is_absolute_even_for_a_relative_argument() {
    let (workspace, config_path) = fixture("relative");
    let nested = workspace.join("sub");
    fs::create_dir_all(&nested).unwrap();

    let reg = register_workspace(
        &config_path,
        &TuiConfig::default(),
        &nested.join(".."),
        None,
    )
    .unwrap();
    assert!(std::path::Path::new(&reg.path).is_absolute());
    // `..` is resolved away, so this is the same entry a plain path would make.
    assert!(!reg.path.contains(".."));
}

#[test]
fn re_registering_the_same_directory_updates_rather_than_duplicates() {
    let (workspace, config_path) = fixture("idempotent");
    let first = register_workspace(&config_path, &TuiConfig::default(), &workspace, None).unwrap();
    assert!(first.added);

    // Second run reads what the first wrote, as the CLI would.
    let loaded = reload(&config_path);
    let second = register_workspace(&config_path, &loaded, &workspace, None).unwrap();

    assert!(!second.added, "an existing path is refreshed, not re-added");
    assert_eq!(second.id, first.id, "the id is stable across re-runs");

    let written = reload(&config_path);
    assert_eq!(written.fleet.workspaces.len(), 1);
    assert_eq!(written.workflow.workspaces.len(), 1);
}

#[test]
fn re_registering_keeps_an_operator_tuned_harness() {
    let (workspace, config_path) = fixture("keepharness");
    register_workspace(
        &config_path,
        &TuiConfig::default(),
        &workspace,
        Some("gpu-box"),
    )
    .unwrap();

    let loaded = reload(&config_path);
    let again = register_workspace(&config_path, &loaded, &workspace, None).unwrap();
    assert_eq!(
        again.harness_id, "gpu-box",
        "a re-run must not silently re-home the workspace"
    );

    // Only an explicit request moves it.
    let loaded = reload(&config_path);
    let moved = register_workspace(&config_path, &loaded, &workspace, Some("laptop")).unwrap();
    assert_eq!(moved.harness_id, "laptop");
}

#[test]
fn a_new_workspace_attaches_to_the_first_declared_harness() {
    let (workspace, config_path) = fixture("declared");
    let mut config = TuiConfig::default();
    config.fleet.harnesses = vec![harness("workstation"), harness("laptop")];

    let reg = register_workspace(&config_path, &config, &workspace, None).unwrap();
    assert_eq!(reg.harness_id, "workstation");
}

#[test]
fn registering_preserves_unrelated_config_and_other_workspaces() {
    let (workspace, config_path) = fixture("preserve");
    fs::write(
        &config_path,
        "stateDir = \"/tmp/keep-me\"\n\n[backend]\nbaseUrl = \"https://example.test\"\n",
    )
    .unwrap();

    let mut config = reload(&config_path);
    config.fleet.workspaces = vec![WorkspaceDescriptor {
        id: "ws-other".into(),
        name: "other".into(),
        path: "/somewhere/else".into(),
        harness_id: "laptop".into(),
        profile: None,
        project: None,
        template_ids: vec!["reviewer".into()],
        metadata: serde_json::Map::new(),
    }];
    config.workflow.workspaces = vec!["/somewhere/else".into()];

    register_workspace(&config_path, &config, &workspace, None).unwrap();

    let written = reload(&config_path);
    assert_eq!(written.state_dir, "/tmp/keep-me");
    assert_eq!(written.backend.base_url, "https://example.test");
    assert_eq!(written.fleet.workspaces.len(), 2);
    // The pre-existing entry survives intact, hand-tuned fields included.
    let other = written
        .fleet
        .workspaces
        .iter()
        .find(|w| w.id == "ws-other")
        .expect("existing workspace kept");
    assert_eq!(other.template_ids, vec!["reviewer".to_string()]);
    assert_eq!(written.workflow.workspaces.len(), 2);
}

#[test]
fn registering_a_missing_directory_is_an_error() {
    let (_, config_path) = fixture("missing");
    let err = register_workspace(
        &config_path,
        &TuiConfig::default(),
        std::path::Path::new("/no/such/directory"),
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("/no/such/directory"));
    // Nothing is written for a path nothing can run in.
    assert!(!config_path.exists());
}

#[test]
fn deregister_removes_from_both_lists_by_path_and_by_id() {
    let (workspace, config_path) = fixture("deregister");
    let reg = register_workspace(&config_path, &TuiConfig::default(), &workspace, None).unwrap();
    fs::write(workspace.join("MEDULLA.md"), "authored").unwrap();

    let removed = deregister_workspace(
        &config_path,
        &reload(&config_path),
        &workspace.display().to_string(),
    )
    .unwrap();
    assert_eq!(removed.id, reg.id);

    let written = reload(&config_path);
    assert!(written.fleet.workspaces.is_empty());
    assert!(written.workflow.workspaces.is_empty());
    // Unregistering must not destroy the operator's authored profile.
    assert_eq!(
        fs::read_to_string(workspace.join("MEDULLA.md")).unwrap(),
        "authored"
    );

    // The same works by registry id.
    register_workspace(&config_path, &reload(&config_path), &workspace, None).unwrap();
    deregister_workspace(&config_path, &reload(&config_path), &reg.id).unwrap();
    assert!(reload(&config_path).fleet.workspaces.is_empty());
}

#[test]
fn deregister_leaves_other_workspaces_alone() {
    let (workspace, config_path) = fixture("deregister-others");
    let mut config = TuiConfig::default();
    config.fleet.workspaces = vec![WorkspaceDescriptor {
        id: "ws-keep".into(),
        name: "keep".into(),
        path: "/somewhere/else".into(),
        harness_id: "laptop".into(),
        profile: None,
        project: None,
        template_ids: Vec::new(),
        metadata: serde_json::Map::new(),
    }];
    config.workflow.workspaces = vec!["/somewhere/else".into()];
    register_workspace(&config_path, &config, &workspace, None).unwrap();

    deregister_workspace(
        &config_path,
        &reload(&config_path),
        &workspace.display().to_string(),
    )
    .unwrap();

    let written = reload(&config_path);
    assert_eq!(written.fleet.workspaces.len(), 1);
    assert_eq!(written.fleet.workspaces[0].id, "ws-keep");
    assert_eq!(
        written.workflow.workspaces,
        vec!["/somewhere/else".to_string()]
    );
}

#[test]
fn deregister_of_an_unknown_selector_is_an_error() {
    let (workspace, config_path) = fixture("deregister-unknown");
    register_workspace(&config_path, &TuiConfig::default(), &workspace, None).unwrap();

    let err = deregister_workspace(&config_path, &reload(&config_path), "ws-not-here").unwrap_err();
    assert!(err.to_string().contains("ws-not-here"));
    // A failed match leaves the registry exactly as it was.
    assert_eq!(reload(&config_path).fleet.workspaces.len(), 1);
}

#[test]
fn deregister_can_remove_a_half_registered_root() {
    // A path in `[workflow].workspaces` but not the fleet chain — hand-edited
    // config, or an older release — must still be removable.
    let (workspace, config_path) = fixture("deregister-half");
    let path = workspace.display().to_string();
    let mut config = TuiConfig::default();
    config.workflow.workspaces = vec![path.clone()];

    deregister_workspace(&config_path, &config, &path).unwrap();
    assert!(reload(&config_path).workflow.workspaces.is_empty());
}

#[test]
fn workspace_ids_are_readable_stable_and_path_unique() {
    let a = workspace_id("/home/dev/medulla-public");
    assert!(a.starts_with("ws-medulla-public-"), "got {a}");
    assert_eq!(a, workspace_id("/home/dev/medulla-public"));

    // Two checkouts of the same repository — the common worktree case — must not
    // collide, or one would silently overwrite the other's registry entry.
    let b = workspace_id("/home/dev/worktrees/medulla-public");
    assert_ne!(a, b);
}
