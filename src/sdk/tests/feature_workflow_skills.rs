#![cfg(feature = "workflows")]

//! Generated harness skills, from the store an operator actually authors into
//! to the files a harness actually reads.
//!
//! The unit tests either side of this build their [`WorkflowSummary`] by hand,
//! so they cannot notice the thing that would break the feature in practice: a
//! workflow authored through [`ops`] whose listing view does not carry what
//! rendering reads, or an edit that changes the graph without changing the
//! skill (or vice versa). This drives the real chain — `ops::create`, a real
//! [`FileWorkflowStore`], `store.list()`, `skills::install` — against a tempdir
//! standing in for `$HOME`, and asserts on the bytes on disk.
//!
//! Offline and process-free: nothing here runs a workflow, and every path is
//! inside a `tempfile::TempDir`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use medulla::workflows::skills::{
    self, FileAction, InstallOptions, InstallReport, SkillScope, SkillTarget,
};
use medulla::workflows::{ops, FileWorkflowStore, WorkflowStore, WorkflowSummary};

/// A store rooted in a scratch directory, with its tempdir kept alive by the
/// caller — dropping it deletes the workflows underneath.
fn store() -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    let root = tempfile::tempdir().expect("a scratch store root");
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    (root, store)
}

/// A runnable one-agent workflow document with whatever inputs it declares.
///
/// `inputs` is passed as JSON rather than typed values because that is how it
/// arrives from every real caller — a document a model or an operator wrote.
fn document(id: &str, description: &str, inputs: Value) -> String {
    json!({
        "id": id,
        "name": id,
        "description": description,
        "inputs": inputs,
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "do it" } }
        ],
        "edges": [{ "from_node": "t", "to_node": "work" }]
    })
    .to_string()
}

/// The listing view the skill renderer consumes, straight from the store.
fn summaries(store: &Arc<dyn WorkflowStore>) -> Vec<WorkflowSummary> {
    store.list().expect("the store lists what was authored")
}

/// Install options over both verified harnesses, commands included, so a
/// regression that only touches one layout still fails.
fn options(home: &Path) -> InstallOptions {
    InstallOptions {
        targets: vec![SkillTarget::Claude, SkillTarget::Codex],
        scope: SkillScope::User,
        root: home.to_path_buf(),
        with_commands: true,
        dry_run: false,
    }
}

/// The four files a two-target, commands-on install writes for one workflow.
fn expected_paths(home: &Path, slug: &str) -> Vec<PathBuf> {
    vec![
        home.join(".claude/skills").join(slug).join("SKILL.md"),
        home.join(".claude/commands").join(format!("{slug}.md")),
        // Codex reads `.agents/skills`, not its own config directory.
        home.join(".agents/skills").join(slug).join("SKILL.md"),
        home.join(".codex/prompts").join(format!("{slug}.md")),
    ]
}

/// What the report says happened to one workflow's files.
fn actions_for<'a>(report: &'a InstallReport, workflow_id: &str) -> Vec<&'a FileAction> {
    report
        .files
        .iter()
        .filter(|file| file.workflow_id == workflow_id)
        .map(|file| &file.action)
        .collect()
}

/// Author two workflows and install their skills into a scratch home.
///
/// Returned as `(store root, store, home, install report)`; both tempdirs are
/// handed back so the caller keeps them alive for the length of the test.
fn authored_and_installed() -> (
    tempfile::TempDir,
    Arc<dyn WorkflowStore>,
    tempfile::TempDir,
    InstallReport,
) {
    let (root, store) = store();
    ops::create(
        &store,
        &document(
            "babysit",
            "Watch a pull request until it merges",
            json!([
                { "name": "pr", "type": "number", "required": true,
                  "description": "The pull request to babysit" }
            ]),
        ),
        "babysit",
    )
    .expect("babysit installs");
    ops::create(
        &store,
        &document("audit", "Audit a repository", json!([])),
        "audit",
    )
    .expect("audit installs");

    let home = tempfile::tempdir().expect("a scratch home");
    let report =
        skills::install(&summaries(&store), &options(home.path())).expect("skills install");
    (root, store, home, report)
}

#[test]
fn authoring_a_workflow_and_installing_its_skill_writes_a_harness_readable_file() {
    let (_root, _store, home, report) = authored_and_installed();

    // Two workflows × two targets × (skill + command).
    assert_eq!(report.count(FileAction::Created), 8, "{report:?}");
    assert!(!report.has_collisions());
    for slug in ["medulla-babysit", "medulla-audit"] {
        for path in expected_paths(home.path(), slug) {
            assert!(path.is_file(), "{} was not written", path.display());
        }
    }

    let skill = fs::read_to_string(home.path().join(".claude/skills/medulla-babysit/SKILL.md"))
        .expect("the claude skill is readable");
    // The marker is what every later write checks before touching this file —
    // carried as a YAML comment inside the frontmatter, so the frontmatter
    // still opens on line 1 and the harness actually parses it.
    assert!(
        skill.starts_with("---\n# medulla:managed workflow=babysit rev="),
        "{skill}"
    );
    assert!(skill.contains("name: medulla-babysit"), "{skill}");
    // The description the operator wrote reaches the frontmatter the model
    // matches on — the whole feature turns on that hop.
    assert!(
        skill.contains("Watch a pull request until it merges."),
        "{skill}"
    );
    // The declared input survives store round-tripping into the summary.
    assert!(
        skill.contains("| `pr` | number | yes | The pull request to babysit | — |"),
        "{skill}"
    );
    assert!(skill.contains("mcp__medulla__workflow_run"), "{skill}");
    assert!(skill.contains("\"id\": \"babysit\""), "{skill}");

    // Rendering is target-independent: Codex gets the same bytes, and a change
    // that made one target's body drift would be a bug, not a feature.
    assert_eq!(
        skill,
        fs::read_to_string(home.path().join(".agents/skills/medulla-babysit/SKILL.md")).unwrap()
    );

    // A workflow with no inputs says so rather than rendering an empty table.
    let audit = fs::read_to_string(home.path().join(".claude/skills/medulla-audit/SKILL.md"))
        .expect("the audit skill is readable");
    assert!(audit.contains("This workflow takes no inputs"), "{audit}");

    // The command variant is the operator-typed door onto the same call.
    let command = fs::read_to_string(home.path().join(".claude/commands/medulla-babysit.md"))
        .expect("the claude command is readable");
    assert!(command.contains("argument-hint: \"<pr>\""), "{command}");
    assert!(command.contains("$ARGUMENTS"), "{command}");
}

#[test]
fn editing_one_workflow_and_syncing_rewrites_only_that_workflows_files() {
    let (_root, store, home, _first) = authored_and_installed();
    let before: Vec<String> = expected_paths(home.path(), "medulla-audit")
        .iter()
        .map(|path| fs::read_to_string(path).expect("installed"))
        .collect();
    let babysit_skill = home.path().join(".claude/skills/medulla-babysit/SKILL.md");
    let babysit_before = fs::read_to_string(&babysit_skill).expect("installed");

    // A real authoring edit, through the same patch language a copilot uses:
    // the workflow gains a second, defaulted input.
    ops::apply_ops(
        &store,
        "babysit",
        &json!({
            "ops": [{
                "op": "set_workflow_inputs",
                "inputs": [
                    { "name": "pr", "type": "number", "required": true,
                      "description": "The pull request to babysit" },
                    { "name": "repo", "type": "string", "default": "acme/api",
                      "description": "Repository the pull request is in" }
                ]
            }]
        }),
    )
    .expect("the edit applies");

    let report = skills::sync(&summaries(&store), &options(home.path()), false).expect("sync");

    // Exactly the edited workflow's files are rewritten; the untouched one is
    // not, which is what makes a sync safe to run on a timer.
    assert_eq!(report.count(FileAction::Updated), 4, "{report:?}");
    assert_eq!(report.count(FileAction::Unchanged), 4, "{report:?}");
    assert!(actions_for(&report, "babysit")
        .iter()
        .all(|action| **action == FileAction::Updated));
    assert!(actions_for(&report, "audit")
        .iter()
        .all(|action| **action == FileAction::Unchanged));

    let babysit_after = fs::read_to_string(&babysit_skill).expect("still installed");
    assert_ne!(babysit_before, babysit_after);
    assert!(
        babysit_after.contains(
            "| `repo` | string | no | Repository the pull request is in | `\"acme/api\"` |"
        ),
        "{babysit_after}"
    );
    // The new default is runnable as written in the example call.
    assert!(
        babysit_after.contains("\"repo\": \"acme/api\""),
        "{babysit_after}"
    );

    let after: Vec<String> = expected_paths(home.path(), "medulla-audit")
        .iter()
        .map(|path| fs::read_to_string(path).expect("untouched"))
        .collect();
    assert_eq!(before, after, "the unedited workflow's bytes must not move");
}

#[test]
fn deleting_a_workflow_and_syncing_with_prune_removes_its_skill_and_nothing_else() {
    let (_root, store, home, _first) = authored_and_installed();
    // A file the operator wrote by hand, in the directory we are pruning.
    let neighbour = home.path().join(".claude/skills/handwritten/SKILL.md");
    fs::create_dir_all(neighbour.parent().unwrap()).unwrap();
    fs::write(&neighbour, "mine, not medulla's\n").unwrap();

    ops::delete(&store, "audit").expect("the workflow is deleted");

    let report = skills::sync(&summaries(&store), &options(home.path()), true).expect("sync");

    // Both targets' skill and command files for the deleted workflow.
    assert_eq!(report.count(FileAction::Removed), 4, "{report:?}");
    assert!(actions_for(&report, "audit")
        .iter()
        .all(|action| **action == FileAction::Removed));
    for path in expected_paths(home.path(), "medulla-audit") {
        assert!(!path.exists(), "{} survived the prune", path.display());
    }
    // The now-empty skill directory goes too: leaving it behind would make an
    // uninstalled skill look installed to anyone reading the tree.
    assert!(!home.path().join(".claude/skills/medulla-audit").exists());

    for path in expected_paths(home.path(), "medulla-babysit") {
        assert!(path.is_file(), "{} should have been kept", path.display());
    }
    assert_eq!(
        fs::read_to_string(&neighbour).unwrap(),
        "mine, not medulla's\n"
    );

    // And what the store now offers matches what is on disk.
    let found = skills::installed(&options(home.path())).expect("listing");
    let ids: Vec<&str> = found
        .iter()
        .map(|skill| skill.workflow_id.as_str())
        .collect();
    assert_eq!(ids, vec!["babysit", "babysit"], "one per target");
}
