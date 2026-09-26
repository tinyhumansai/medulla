//! Tests for Medulla's half of the store: where the catalog lands, and what a
//! `defaults` block is allowed to say.
//!
//! The store's own behaviour — layered reads, atomic writes, revisions, the
//! journal, proposal fingerprints — is tested in `tinyflows::store`, next to the
//! code. What is checked here is only what this crate contributes: the home
//! layout, and the harness rule the engine cannot judge on its own.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::*;
use crate::workflows::WorkflowStore;

/// The smallest document that validates.
fn valid_document(id: &str) -> String {
    json!({
        "id": id,
        "name": "Greet",
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "greet", "kind": "transform", "name": "greet",
              "config": { "set": { "greeting": "=.item.name" } } }
        ],
        "edges": [
            { "from_node": "t", "from_port": "main", "to_node": "greet", "to_port": "main" }
        ]
    })
    .to_string()
}

#[test]
fn the_directories_are_project_then_home_with_home_as_the_write_layer() {
    let env = HashMap::from([("MEDULLA_HOME".to_string(), "/somewhere/home".to_string())]);

    let dirs = workflow_dirs(&env, Path::new("/repo"));

    assert_eq!(
        dirs,
        vec![
            PathBuf::from("/repo/.medulla/workflows"),
            // The account's home, not the root: `MEDULLA_HOME` names the
            // directory that holds accounts, and a signed-out run is `local`.
            PathBuf::from("/somewhere/home/local/workflows"),
        ]
    );
}

#[test]
fn a_dev_home_sits_below_the_projects_own_workflow_directory() {
    // Under MEDULLA_DEV the root is ./.medulla, but the home is the account
    // directory inside it — so the repository's checked-in `.medulla/workflows`
    // and the dev write layer are two real directories, not one seen twice.
    let env = HashMap::from([("MEDULLA_DEV".to_string(), "1".to_string())]);

    let dirs = workflow_dirs(&env, Path::new("."));

    assert_eq!(
        dirs,
        vec![
            PathBuf::from("./.medulla/workflows"),
            PathBuf::from(".medulla/local/workflows"),
        ]
    );
}

#[test]
fn a_discovered_store_saves_new_definitions_under_medulla_home() {
    let root = tempfile::tempdir().unwrap();
    let medulla_root = root.path().join("home");
    let project = root.path().join("project");
    let env = HashMap::from([(
        "MEDULLA_HOME".to_string(),
        medulla_root.to_string_lossy().into_owned(),
    )]);
    let home = crate::home::medulla_home(&env);
    let store = discover(&env, &project);
    let record = parse_workflow(&valid_document("home-save"), "home-save").unwrap();

    store.save(&record).unwrap();

    assert!(home.join("workflows/home-save.json").is_file());
    assert!(!project.join(".medulla/workflows/home-save.json").exists());
}

#[test]
fn a_discovered_store_refuses_a_defaults_block_naming_something_that_cannot_be_a_harness() {
    // The reason `MedullaPolicy` exists. The engine treats `defaults.harness`
    // as an opaque string, so without this policy the document would load and
    // the workflow would quietly run on the host default instead.
    let root = tempfile::tempdir().unwrap();
    let env = HashMap::from([(
        "MEDULLA_HOME".to_string(),
        root.path().join("home").to_string_lossy().into_owned(),
    )]);
    let store = discover(&env, &root.path().join("project"));
    let document = json!({
        "id": "nightly",
        "defaults": { "harness": "claude code" },
        "nodes": [{ "id": "t", "kind": "trigger", "name": "start" }],
        "edges": []
    })
    .to_string();
    let dir = crate::home::medulla_home(&env).join("workflows");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("nightly.json"), document).unwrap();

    // Reported rather than silently skipped: an operator whose catalog stopped
    // listing a workflow needs to be told which file and why.
    let report = store.load();

    assert!(report.workflows.is_empty(), "{:?}", report.workflows);
    let joined = report.errors.join("; ");
    assert!(joined.contains("defaults"), "{joined}");
    assert!(joined.contains("custom harness id"), "{joined}");
}

#[test]
fn a_defaults_block_naming_a_known_harness_becomes_a_preference() {
    let defaults = WorkflowDefaults {
        harness: Some("codex".into()),
        model: Some(" gpt-5-codex ".into()),
    };

    let preference = preference(&defaults).expect("a known harness");

    assert!(preference.harness.is_some());
    // Trimmed, because an operator's stray space is not part of the model name.
    assert_eq!(preference.model.as_deref(), Some("gpt-5-codex"));
}
