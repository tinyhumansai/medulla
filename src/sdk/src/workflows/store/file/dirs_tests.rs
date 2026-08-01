//! Unit tests for workflow directory layering: which directories hold
//! workflows, in what precedence, and how the account-scoped home relates to a
//! repository's own checked-in store.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::super::{parse_workflow, FileWorkflowStore};
use super::workflow_dirs;
use crate::workflows::WorkflowStore;

/// The smallest document that validates: one trigger, one transform, one edge.
fn valid_document(id: &str) -> String {
    serde_json::json!({
        "id": id,
        "name": "Greet",
        "description": "says hello",
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
fn a_discovered_store_saves_new_definitions_under_medulla_home() {
    let root = tempfile::tempdir().unwrap();
    let medulla_root = root.path().join("home");
    let project = root.path().join("project");
    let env = HashMap::from([(
        "MEDULLA_HOME".to_string(),
        medulla_root.to_string_lossy().into_owned(),
    )]);
    let home = crate::home::medulla_home(&env);
    let store = FileWorkflowStore::discover(&env, &project);
    let record = parse_workflow(&valid_document("home-save"), "home-save").unwrap();

    store.save(&record).unwrap();

    assert!(home.join("workflows/home-save.json").is_file());
    assert!(!project.join(".medulla/workflows/home-save.json").exists());
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
