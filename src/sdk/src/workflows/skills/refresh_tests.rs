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

/// Two workspaces do not share a catalog, so they must not share a managed
/// directory either.
///
/// `discover` layers `<cwd>/.medulla/workflows` under the user-global one, so a
/// project-local workflow belongs to exactly one project. With one
/// account-wide directory the second refresh would prune the first project's
/// skill — the operator's other session would silently lose it — and, worse,
/// each session would be handed the other project's skills for as long as the
/// window between the two passes lasted.
#[test]
fn two_workspaces_do_not_prune_or_read_each_others_skills() {
    let home = TempDir::new().unwrap();
    let one = TempDir::new().unwrap();
    let two = TempDir::new().unwrap();
    let env = env_at(home.path());

    write_workflow(&env, "shared", "Everyone's.", true);
    write_project_workflow(one.path(), "only-one", "First project's.", true);
    write_project_workflow(two.path(), "only-two", "Second project's.", true);

    let args_one = refresh_managed(HarnessProvider::Claude, &env, one.path());
    let args_two = refresh_managed(HarnessProvider::Claude, &env, two.path());

    assert_ne!(
        args_one, args_two,
        "each workspace must be pointed at its own directory"
    );

    // The second refresh must not have taken the first's skill with it.
    assert!(managed_skill(&env, one.path(), "only-one").is_file());
    assert!(managed_skill(&env, two.path(), "only-two").is_file());

    // And neither session can see the other project's workflow at all.
    assert!(
        !managed_skill(&env, one.path(), "only-two").exists(),
        "a session must never be handed another project's skills"
    );
    assert!(
        !managed_skill(&env, two.path(), "only-one").exists(),
        "a session must never be handed another project's skills"
    );

    // The user-global layer is still in both, since it is in both catalogs.
    assert!(managed_skill(&env, one.path(), "shared").is_file());
    assert!(managed_skill(&env, two.path(), "shared").is_file());
}

/// Concurrent refreshes of one workspace serialize rather than interleave.
///
/// The failure this rules out is a lost skill: a pass that read the directory
/// before a second pass wrote to it would find that file's workflow absent from
/// its own listing and prune it away. The refreshes here run against a store
/// that changes underneath them, which is the shape that produces two
/// disagreeing listings in the first place.
#[test]
fn concurrent_refreshes_leave_the_directory_matching_the_store() {
    use std::sync::{Arc, Barrier};

    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let env = env_at(home.path());
    write_workflow(&env, "keeper", "Stays throughout.", true);

    // Released together so the passes genuinely overlap rather than happening
    // to run one after another by scheduling luck.
    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let env = env.clone();
            let cwd = cwd.path().to_path_buf();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..8 {
                    sync_managed(SkillTarget::Claude, &env, &cwd).expect("refresh succeeds");
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("no refresh panicked");
    }

    // One last pass with nothing else running fixes the store state the answer
    // is checked against.
    sync_managed(SkillTarget::Claude, &env, cwd.path()).unwrap();
    assert!(
        managed_skill(&env, cwd.path(), "keeper").is_file(),
        "no pass may prune a skill whose workflow is still in the store"
    );
}

/// The lock is exclusive across processes, not merely across threads sharing
/// one guard: a refresh blocks while another holds the workspace's lock file.
#[test]
fn a_refresh_waits_for_the_workspace_lock() {
    use std::sync::mpsc;
    use std::time::Duration;

    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let env = env_at(home.path());
    write_workflow(&env, "babysit", "Watch a PR.", true);

    let held = refresh::RefreshLock::acquire(&managed_root(&env, cwd.path())).unwrap();

    let (tx, rx) = mpsc::channel();
    let worker = {
        let env = env.clone();
        let cwd = cwd.path().to_path_buf();
        std::thread::spawn(move || {
            sync_managed(SkillTarget::Claude, &env, &cwd).expect("refresh succeeds");
            let _ = tx.send(());
        })
    };

    assert!(
        rx.recv_timeout(Duration::from_millis(250)).is_err(),
        "a refresh must not proceed while another holds the workspace lock"
    );
    assert!(
        !managed_skill(&env, cwd.path(), "babysit").exists(),
        "and it must not have written anything either"
    );

    drop(held);
    rx.recv_timeout(Duration::from_secs(10))
        .expect("the refresh proceeds once the lock is released");
    worker.join().unwrap();
    assert!(managed_skill(&env, cwd.path(), "babysit").is_file());
}

/// Skills a pre-0.9 release left in the unscoped root are retired, since
/// nothing points a harness at that directory any more.
#[test]
fn the_unscoped_managed_root_is_retired() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let env = env_at(home.path());
    write_workflow(&env, "babysit", "Watch a PR.", true);

    // What the old layout wrote: the same install, at the Medulla home itself.
    let legacy = crate::home::medulla_home(&env).join("claude-skills");
    install(
        &[
            crate::workflows::store::FileWorkflowStore::discover(&env, cwd.path())
                .load()
                .workflows[0]
                .summary(),
        ],
        &InstallOptions {
            targets: vec![SkillTarget::Claude],
            scope: SkillScope::Project,
            root: legacy.clone(),
            with_commands: false,
            dry_run: false,
        },
    )
    .unwrap();
    let stranded = skill_path(SkillTarget::Claude, &legacy, &slug_for("babysit"));
    assert!(stranded.is_file(), "fixture must reproduce the old layout");

    // A file the operator put there themselves, to prove the removal is still
    // marker-gated.
    let theirs = skill_path(SkillTarget::Claude, &legacy, "hand-written");
    fs::create_dir_all(theirs.parent().unwrap()).unwrap();
    fs::write(&theirs, "---\nname: mine\n---\n").unwrap();

    refresh_managed(HarnessProvider::Claude, &env, cwd.path());

    assert!(
        !stranded.exists(),
        "a skill nothing reads any more must not be left behind"
    );
    assert!(theirs.is_file(), "a file Medulla did not generate stays");
    assert!(managed_skill(&env, cwd.path(), "babysit").is_file());
}
