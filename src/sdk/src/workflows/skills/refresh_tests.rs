//! Unit tests for the spawn-time refresh of the managed skills.
//!
//! What is being proved here is a single property the install pipeline alone
//! could not give: that the skills a spawned harness reads describe the
//! workflow store *as it is at that moment*, without anyone re-running
//! `medulla skills install`. So every test moves the store between two spawns
//! and asserts the second spawn sees the move.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::json;
use tempfile::TempDir;

use crate::protocol::HarnessProvider;

use super::*;

/// The smallest document the store will load: one trigger, one transform.
fn document(id: &str, description: &str, enabled: bool) -> String {
    json!({
        "id": id,
        "name": id,
        "description": description,
        "enabled": enabled,
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

/// Put a workflow document in the home store, where `discover` reads it.
fn write_workflow(env: &HashMap<String, String>, id: &str, description: &str, enabled: bool) {
    let dir = workflows_dir(env);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{id}.json")),
        document(id, description, enabled),
    )
    .unwrap();
}

/// The store directory the documents above land in: the user-global layer, at
/// the resolved Medulla home rather than at `MEDULLA_HOME` itself, because the
/// home is per-account (`<root>/<user id>`) and the store reads the resolved
/// one.
fn workflows_dir(env: &HashMap<String, String>) -> std::path::PathBuf {
    crate::home::medulla_home(env).join("workflows")
}

/// The project-local layer for `cwd`, which is what makes two workspaces have
/// two catalogs.
fn project_workflows_dir(cwd: &Path) -> std::path::PathBuf {
    cwd.join(".medulla").join("workflows")
}

/// Put a workflow document in `cwd`'s project-local store.
fn write_project_workflow(cwd: &Path, id: &str, description: &str, enabled: bool) {
    let dir = project_workflows_dir(cwd);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{id}.json")),
        document(id, description, enabled),
    )
    .unwrap();
}

/// An environment whose Medulla home is the scratch directory.
fn env_at(home: &Path) -> HashMap<String, String> {
    HashMap::from([("MEDULLA_HOME".to_string(), home.display().to_string())])
}

/// The managed `SKILL.md` for `id` in `cwd`'s scope, whether or not it exists.
fn managed_skill(env: &HashMap<String, String>, cwd: &Path, id: &str) -> std::path::PathBuf {
    skill_path(
        SkillTarget::Claude,
        &managed_dir(SkillTarget::Claude, env, cwd),
        &slug_for(id),
    )
}

/// The headline case: a workflow authored after the last install is described
/// to the very next session, and the argv points at it.
#[test]
fn a_workflow_authored_since_the_last_install_reaches_the_next_session() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let env = env_at(home.path());

    // Nothing saved yet: no skills, and no reason to grant the session a
    // directory.
    assert!(refresh_managed(HarnessProvider::Claude, &env, cwd.path()).is_empty());

    write_workflow(&env, "babysit", "Watch a PR.", true);

    let args = refresh_managed(HarnessProvider::Claude, &env, cwd.path());
    assert_eq!(
        args,
        vec![
            "--add-dir".to_string(),
            managed_dir(SkillTarget::Claude, &env, cwd.path())
                .display()
                .to_string()
        ],
        "once the store has a workflow the session must be pointed at the skills"
    );
    let body = fs::read_to_string(managed_skill(&env, cwd.path(), "babysit")).unwrap();
    assert!(body.contains("Watch a PR."), "got {body}");
}

/// An edit to a workflow rewrites its skill rather than leaving the first
/// render in place — the stale-description half of the bug.
#[test]
fn an_edited_workflow_rewrites_its_skill() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let env = env_at(home.path());

    write_workflow(&env, "babysit", "Watch a PR.", true);
    refresh_managed(HarnessProvider::Claude, &env, cwd.path());

    write_workflow(&env, "babysit", "Watch a PR until it is green.", true);
    refresh_managed(HarnessProvider::Claude, &env, cwd.path());

    let body = fs::read_to_string(managed_skill(&env, cwd.path(), "babysit")).unwrap();
    assert!(body.contains("until it is green"), "got {body}");
}

/// Deleting or disabling a workflow retires its skill. This is the half no
/// amount of re-running `install` would have fixed: a harness kept being told
/// about a workflow it could no longer run.
#[test]
fn a_deleted_or_disabled_workflow_loses_its_skill() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let env = env_at(home.path());

    write_workflow(&env, "babysit", "Watch a PR.", true);
    write_workflow(&env, "triage", "Triage issues.", true);
    refresh_managed(HarnessProvider::Claude, &env, cwd.path());
    assert!(managed_skill(&env, cwd.path(), "babysit").is_file());
    assert!(managed_skill(&env, cwd.path(), "triage").is_file());

    fs::remove_file(workflows_dir(&env).join("babysit.json")).unwrap();
    write_workflow(&env, "triage", "Triage issues.", false);
    refresh_managed(HarnessProvider::Claude, &env, cwd.path());

    assert!(
        !managed_skill(&env, cwd.path(), "babysit").exists(),
        "a deleted workflow must stop being advertised"
    );
    assert!(
        !managed_skill(&env, cwd.path(), "triage").exists(),
        "a disabled workflow must not be offered as runnable"
    );
}

/// A malformed document suspends pruning. The store drops what it cannot parse,
/// so pruning on that listing would delete a good skill because of an unrelated
/// broken file — and the operator would find their catalog thinned by a typo.
#[test]
fn a_store_that_failed_to_load_does_not_prune() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let env = env_at(home.path());

    write_workflow(&env, "babysit", "Watch a PR.", true);
    refresh_managed(HarnessProvider::Claude, &env, cwd.path());
    assert!(managed_skill(&env, cwd.path(), "babysit").is_file());

    // Mid-edit: the document no longer parses, so the listing no longer
    // mentions the workflow.
    fs::write(workflows_dir(&env).join("babysit.json"), "{ not json").unwrap();
    refresh_managed(HarnessProvider::Claude, &env, cwd.path());

    assert!(
        managed_skill(&env, cwd.path(), "babysit").is_file(),
        "a skill must survive a parse failure elsewhere in the store"
    );
}

/// Only Medulla's own root is written. An operator's `~/.claude` is theirs, and
/// an automatic refresh must never reach into it.
#[test]
fn the_operators_own_claude_directory_is_untouched() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let mut env = env_at(home.path());
    env.insert("HOME".to_string(), home.path().display().to_string());
    let curated = home.path().join(".claude").join("skills");
    fs::create_dir_all(&curated).unwrap();

    write_workflow(&env, "babysit", "Watch a PR.", true);
    refresh_managed(HarnessProvider::Claude, &env, cwd.path());

    assert!(
        fs::read_dir(&curated).unwrap().next().is_none(),
        "the user-scope skills directory must be left exactly as the operator keeps it"
    );
}

/// A provider Medulla cannot point at a directory gets neither argv nor a
/// directory full of files nothing will read.
#[test]
fn a_provider_without_a_directory_flag_is_left_alone() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let env = env_at(home.path());
    write_workflow(&env, "babysit", "Watch a PR.", true);

    assert!(refresh_managed(HarnessProvider::Codex, &env, cwd.path()).is_empty());
    assert!(
        !managed_dir(SkillTarget::Codex, &env, cwd.path()).exists(),
        "nothing reads a managed Codex directory today, so nothing should write one"
    );
}
