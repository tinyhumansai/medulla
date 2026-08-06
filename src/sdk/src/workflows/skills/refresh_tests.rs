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

/// Four threads refreshing one workspace at once, released together so the
/// passes genuinely overlap rather than running one after another by scheduling
/// luck.
fn refresh_burst(env: &HashMap<String, String>, cwd: &Path) {
    use std::sync::{Arc, Barrier};

    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let env = env.clone();
            let cwd = cwd.to_path_buf();
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
}

/// Concurrent refreshes of one workspace serialize rather than interleave, and
/// what they leave behind is the store, not a partial view of it.
///
/// The failure this rules out is a lost skill: a pass that read the directory
/// before a second pass wrote to it would find that file's workflow absent from
/// its own listing and prune it away. So the store is moved between bursts —
/// added to, then taken from — and the directory is read straight after each
/// burst, with no corrective pass to paper over a race. Every pass in a burst
/// starts after that burst's change, so a burst that serializes correctly can
/// only end in one state: the one the store describes.
#[test]
fn concurrent_refreshes_leave_the_directory_matching_the_store() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let env = env_at(home.path());
    write_workflow(&env, "keeper", "Stays throughout.", true);

    refresh_burst(&env, cwd.path());
    assert!(
        managed_skill(&env, cwd.path(), "keeper").is_file(),
        "no pass may prune a skill whose workflow is still in the store"
    );

    // Authored while the previous burst's files are already on disk: the new
    // skill must survive every other pass's prune.
    write_workflow(&env, "late", "Written between bursts.", true);
    refresh_burst(&env, cwd.path());
    assert!(managed_skill(&env, cwd.path(), "late").is_file());
    assert!(managed_skill(&env, cwd.path(), "keeper").is_file());

    // And a deletion reaches the directory just as reliably: the racing passes
    // must converge on the smaller catalog rather than one of them reinstating
    // the skill from a listing it took earlier.
    fs::remove_file(workflows_dir(&env).join("keeper.json")).unwrap();
    refresh_burst(&env, cwd.path());
    assert!(
        !managed_skill(&env, cwd.path(), "keeper").exists(),
        "a deleted workflow's skill must not survive a burst of refreshes"
    );
    assert!(managed_skill(&env, cwd.path(), "late").is_file());
}

/// The environment variable that turns the helper test below into a lock
/// holder, carrying the managed root to lock.
const HOLD_LOCK_ROOT: &str = "MEDULLA_TEST_HOLD_LOCK_ROOT";

/// Printed by the holder once it owns the lock, so the parent never has to
/// guess when to start timing.
const HOLD_LOCK_READY: &str = "medulla-test: lock held";

/// The holder's full test path, as the re-executed binary's filter needs it.
const HOLD_LOCK_TEST: &str = "workflows::skills::refresh_tests::holds_the_workspace_lock";

/// Not a test of its own: the other half of
/// [`a_refresh_waits_for_the_workspace_lock`], which re-executes this binary
/// with [`HOLD_LOCK_ROOT`] set to make a *second process* hold the lock. A
/// normal run has no such variable and this returns immediately.
///
/// The lock is held until the parent closes this process's stdin, so the
/// release is an event the parent chooses rather than a sleep either side has
/// to guess at.
#[test]
fn holds_the_workspace_lock() {
    use std::io::{Read, Write};

    let Ok(root) = std::env::var(HOLD_LOCK_ROOT) else {
        return;
    };

    let held = refresh::RefreshLock::acquire(Path::new(&root)).expect("the holder takes the lock");
    println!("{HOLD_LOCK_READY}");
    std::io::stdout().flush().unwrap();

    let mut sink = String::new();
    std::io::stdin().read_to_string(&mut sink).unwrap();
    drop(held);
}

/// The lock is exclusive across processes, not merely across threads sharing
/// one guard: a refresh blocks while another *process* holds the workspace's
/// lock file, and completes once that process lets go.
///
/// Two Medulla instances on one machine are the case that matters — a TUI and a
/// headless executor in the same checkout — and a same-process contender could
/// pass on nothing more than Rust's own borrow of the guard, so the contender
/// here is a re-execution of this test binary.
#[test]
fn a_refresh_waits_for_the_workspace_lock() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let env = env_at(home.path());
    write_workflow(&env, "babysit", "Watch a PR.", true);

    let mut holder = Command::new(std::env::current_exe().unwrap())
        .args([HOLD_LOCK_TEST, "--exact", "--nocapture"])
        .env(HOLD_LOCK_ROOT, managed_root(&env, cwd.path()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("the test binary re-executes");
    let mut announcements = BufReader::new(holder.stdout.take().unwrap()).lines();
    let announced = announcements
        .by_ref()
        .map(|line| line.expect("the holder's output is readable"))
        .any(|line| line == HOLD_LOCK_READY);
    assert!(announced, "the holder must announce that it has the lock");

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
        "a refresh must not proceed while another process holds the workspace lock"
    );
    assert!(
        !managed_skill(&env, cwd.path(), "babysit").exists(),
        "and it must not have written anything either"
    );

    // Closing the holder's stdin is what releases the lock.
    drop(holder.stdin.take());
    let status = holder.wait().expect("the holder exits");
    assert!(status.success(), "the holder must not fail: {status}");

    rx.recv_timeout(Duration::from_secs(30))
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

/// A refresh on a multi-thread runtime goes through
/// [`tokio::task::block_in_place`], which panics if it is ever reached from a
/// current-thread runtime. Both flavours are exercised here so a wrong guard
/// fails a test rather than a spawn.
#[test]
fn a_refresh_runs_under_either_runtime_flavour() {
    for multi_thread in [true, false] {
        let home = TempDir::new().unwrap();
        let cwd = TempDir::new().unwrap();
        let env = env_at(home.path());
        write_workflow(&env, "babysit", "Watch a PR.", true);

        let runtime = if multi_thread {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .build()
                .unwrap()
        } else {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
        };
        let args = runtime.block_on(async {
            // Inside a task, which is where every real caller sits.
            tokio::spawn({
                let env = env.clone();
                let cwd = cwd.path().to_path_buf();
                async move { refresh_managed(HarnessProvider::Claude, &env, &cwd) }
            })
            .await
            .unwrap()
        });

        assert!(!args.is_empty(), "multi_thread={multi_thread}");
        assert!(managed_skill(&env, cwd.path(), "babysit").is_file());
    }
}

/// A rewritten skill is replaced whole, so a harness reading the directory
/// while a refresh runs never sees a truncated file — and no temp file is left
/// sitting in the skills root pretending to be one.
#[test]
fn a_rewritten_skill_is_replaced_whole_and_leaves_no_scratch_file() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let env = env_at(home.path());

    write_workflow(&env, "babysit", "Watch a PR.", true);
    refresh_managed(HarnessProvider::Claude, &env, cwd.path());
    let path = managed_skill(&env, cwd.path(), "babysit");
    let first = fs::read_to_string(&path).unwrap();

    // A described-differently workflow renders a different body, so this is an
    // in-place rewrite of an existing file rather than a fresh create.
    write_workflow(&env, "babysit", "Watch a pull request to the end.", true);
    refresh_managed(HarnessProvider::Claude, &env, cwd.path());

    let second = fs::read_to_string(&path).unwrap();
    assert_ne!(first, second, "the fixture must actually rewrite the file");
    assert!(second.contains("Watch a pull request to the end."));

    let strays: Vec<_> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "SKILL.md")
        .collect();
    assert!(strays.is_empty(), "left behind {strays:?}");
}
