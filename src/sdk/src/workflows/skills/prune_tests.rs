//! Unit tests for `sync --prune`: which files a prune pass removes, and — the
//! part that carries the weight — which it must leave alone.
//!
//! Kept apart from [`super::tests`], which covers the install pipeline, because
//! pruning is the only path in this feature that deletes an operator's files.
//! Every rule that narrows it earns a test here: the `medulla-` namespace, a
//! slug a live workflow still claims, Codex's deprecated root, and the symlink
//! a shared skill collection is linked in through.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::workflows::WorkflowSummary;

use super::render::parse_marker;
use super::*;

/// A listing view with the fields skill rendering actually reads.
fn summary(id: &str, description: &str) -> WorkflowSummary {
    WorkflowSummary {
        id: id.to_string(),
        name: id.to_string(),
        description: description.to_string(),
        enabled: true,
        node_count: 3,
        trigger_kind: Some("manual".to_string()),
        inputs: Vec::new(),
    }
}

/// Install options rooted at a scratch directory, writing skills only.
fn opts(root: &Path, targets: Vec<SkillTarget>) -> InstallOptions {
    InstallOptions {
        targets,
        scope: SkillScope::User,
        root: root.to_path_buf(),
        with_commands: false,
        dry_run: false,
    }
}

#[test]
fn sync_prune_removes_orphans_and_spares_unmarked_neighbours() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);
    let kept = summary("babysit", "Watch a PR.");
    let orphan = summary("audit", "Audit a repo.");
    install(&[kept.clone(), orphan], &options).unwrap();

    let neighbour = home.path().join(".claude/skills/handwritten/SKILL.md");
    fs::create_dir_all(neighbour.parent().unwrap()).unwrap();
    fs::write(&neighbour, "mine\n").unwrap();

    let report = sync(&[kept], &options, true).unwrap();

    assert_eq!(report.count(FileAction::Removed), 1);
    let removed = &report
        .files
        .iter()
        .find(|file| file.action == FileAction::Removed)
        .unwrap()
        .workflow_id;
    assert_eq!(removed, "audit");
    assert!(!home.path().join(".claude/skills/medulla-audit").exists());
    assert!(skill_path(SkillTarget::Claude, home.path(), "medulla-babysit").is_file());
    assert_eq!(fs::read_to_string(&neighbour).unwrap(), "mine\n");
}

/// The leftover an operator actually hits: a `medulla-*` skill from a release
/// whose marker this build cannot read. Nothing identifies it as ours except
/// the prefix, and without prefix-pruning no command could ever remove it —
/// the harness would go on offering a workflow that no longer exists.
#[test]
fn sync_prune_removes_an_unreadable_medulla_skill_with_no_workflow_behind_it() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);
    let kept = summary("babysit", "Watch a PR.");
    install(std::slice::from_ref(&kept), &options).unwrap();

    let stale = skill_path(SkillTarget::Claude, home.path(), "medulla-ancient");
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::write(&stale, "---\nname: medulla-ancient\n---\n\nold body\n").unwrap();
    assert!(
        parse_marker(&fs::read_to_string(&stale).unwrap()).is_none(),
        "the fixture is only meaningful if the marker is unreadable"
    );

    let report = sync(&[kept], &options, true).unwrap();

    assert_eq!(report.count(FileAction::Removed), 1);
    assert!(!home.path().join(".claude/skills/medulla-ancient").exists());
    assert!(skill_path(SkillTarget::Claude, home.path(), "medulla-babysit").is_file());
}

/// The other half of the prefix rule: a slug a *kept* workflow claims stays
/// under the marker discipline. Unmanaged content there is a collision the
/// operator resolves, never a file prune deletes behind their back.
#[test]
fn sync_prune_spares_unmanaged_content_at_a_live_workflows_slug() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);
    let kept = summary("babysit", "Watch a PR.");

    let path = skill_path(SkillTarget::Claude, home.path(), "medulla-babysit");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "mine\n").unwrap();

    let report = sync(&[kept], &options, true).unwrap();

    assert_eq!(report.count(FileAction::Removed), 0);
    assert_eq!(report.count(FileAction::SkippedUnmanaged), 1);
    assert_eq!(fs::read_to_string(&path).unwrap(), "mine\n");
}

/// Prefix-pruning is scoped to the namespace: an unmarked skill outside it
/// belongs to someone else however stale it looks.
#[test]
fn sync_prune_leaves_unmarked_skills_outside_the_medulla_namespace() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);
    let neighbour = home.path().join(".claude/skills/handwritten/SKILL.md");
    fs::create_dir_all(neighbour.parent().unwrap()).unwrap();
    fs::write(&neighbour, "mine\n").unwrap();

    let report = sync(&[], &options, true).unwrap();

    assert_eq!(report.count(FileAction::Removed), 0);
    assert_eq!(fs::read_to_string(&neighbour).unwrap(), "mine\n");
}

/// A command file is namespaced the same way, and prunes the same way.
#[test]
fn sync_prune_removes_an_unreadable_medulla_command_file() {
    let home = TempDir::new().unwrap();
    let mut options = opts(home.path(), vec![SkillTarget::Claude]);
    options.with_commands = true;
    let stale = home.path().join(".claude/commands/medulla-ancient.md");
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::write(&stale, "old command\n").unwrap();

    let report = sync(&[], &options, true).unwrap();

    assert_eq!(report.count(FileAction::Removed), 1);
    assert!(!stale.exists());
}

/// `install` retires a legacy Codex duplicate only on behalf of a workflow it
/// is installing, so a deleted workflow's copy is the prune pass's job.
#[test]
fn sync_prune_sweeps_the_deprecated_codex_skills_root() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Codex]);
    let legacy = home.path().join(".codex/skills/medulla-audit/SKILL.md");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&legacy, "---\nname: medulla-audit\n---\n\nold body\n").unwrap();

    let report = sync(&[summary("babysit", "Watch a PR.")], &options, true).unwrap();

    assert_eq!(report.count(FileAction::Removed), 1);
    assert!(!home.path().join(".codex/skills/medulla-audit").exists());
}

/// The legacy sweep must not depend on the order the harnesses were named.
/// `--harness generic,codex` collapses to Generic alone, because both write
/// `.agents/skills` — but `.codex/skills` is Codex's own root, and leftovers
/// there would survive purely because Generic happened to be listed first.
#[test]
fn sync_prune_sweeps_the_deprecated_codex_root_whatever_the_target_order() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Generic, SkillTarget::Codex]);
    let legacy = home.path().join(".codex/skills/medulla-audit/SKILL.md");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&legacy, "---\nname: medulla-audit\n---\n\nold body\n").unwrap();

    let report = sync(&[summary("babysit", "Watch a PR.")], &options, true).unwrap();

    assert_eq!(report.count(FileAction::Removed), 1);
    assert!(!home.path().join(".codex/skills/medulla-audit").exists());
}

/// The legacy root is swept exactly once even when Codex survives dedupe, so a
/// leftover there is not reported (or removed) twice.
#[test]
fn sync_prune_sweeps_the_deprecated_codex_root_only_once() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Codex, SkillTarget::Generic]);
    let legacy = home.path().join(".codex/skills/medulla-audit/SKILL.md");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&legacy, "old body\n").unwrap();

    let report = sync(&[], &options, true).unwrap();

    assert_eq!(report.count(FileAction::Removed), 1);
    assert!(!home.path().join(".codex/skills/medulla-audit").exists());
}

/// An operator who symlinks a shared skill collection into `~/.claude/skills`
/// must not have its files deleted. `read_dir` resolves the link, so the
/// `SKILL.md` behind a `medulla-*` symlink is content outside the root — and
/// the prefix alone is far too thin a claim to delete a file on.
#[cfg(unix)]
#[test]
fn sync_prune_never_deletes_through_a_symlinked_skill_directory() {
    let home = TempDir::new().unwrap();
    let elsewhere = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);

    let shared = elsewhere.path().join("shared-skill");
    fs::create_dir_all(&shared).unwrap();
    let external = shared.join("SKILL.md");
    fs::write(&external, "theirs\n").unwrap();

    let skills = home.path().join(".claude/skills");
    fs::create_dir_all(&skills).unwrap();
    std::os::unix::fs::symlink(&shared, skills.join("medulla-shared")).unwrap();

    let report = sync(&[], &options, true).unwrap();

    assert_eq!(report.count(FileAction::Removed), 0);
    assert_eq!(
        fs::read_to_string(&external).unwrap(),
        "theirs\n",
        "the linked-to file must survive a prune"
    );
    assert!(
        skills.join("medulla-shared").is_symlink(),
        "and so must the link itself"
    );
}

/// The symlink guard is scoped to the prefix rule. A marker we wrote identifies
/// the file wherever it sits, so a marked leftover reached through a link is
/// still ours to retire.
#[cfg(unix)]
#[test]
fn sync_prune_still_removes_a_marked_skill_behind_a_symlink() {
    let home = TempDir::new().unwrap();
    let elsewhere = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);

    let shared = elsewhere.path().join("ours");
    fs::create_dir_all(&shared).unwrap();
    let external = shared.join("SKILL.md");
    fs::write(&external, render(&summary("audit", "Audit a repo.")).body).unwrap();

    let skills = home.path().join(".claude/skills");
    fs::create_dir_all(&skills).unwrap();
    std::os::unix::fs::symlink(&shared, skills.join("medulla-audit")).unwrap();

    let report = sync(&[], &options, true).unwrap();

    assert_eq!(report.count(FileAction::Removed), 1);
    assert!(!external.exists());
}

/// A dry run decides the same removals and writes none of them.
#[test]
fn a_dry_run_prune_reports_the_prefix_removal_without_making_it() {
    let home = TempDir::new().unwrap();
    let mut options = opts(home.path(), vec![SkillTarget::Claude]);
    let stale = skill_path(SkillTarget::Claude, home.path(), "medulla-ancient");
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::write(&stale, "old body\n").unwrap();

    options.dry_run = true;
    let report = sync(&[], &options, true).unwrap();

    assert_eq!(report.count(FileAction::Removed), 1);
    assert!(stale.is_file());
}

#[test]
fn sync_without_prune_leaves_orphans_in_place() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);
    let kept = summary("babysit", "Watch a PR.");
    install(&[kept.clone(), summary("audit", "Audit.")], &options).unwrap();

    let report = sync(&[kept], &options, false).unwrap();

    assert_eq!(report.count(FileAction::Removed), 0);
    assert!(skill_path(SkillTarget::Claude, home.path(), "medulla-audit").is_file());
}

#[test]
fn sync_prunes_a_workflow_that_has_since_been_disabled() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);
    install(&[summary("babysit", "Watch a PR.")], &options).unwrap();

    let mut disabled = summary("babysit", "Watch a PR.");
    disabled.enabled = false;
    let report = sync(&[disabled], &options, true).unwrap();

    assert_eq!(report.count(FileAction::Removed), 1);
    assert!(!skill_path(SkillTarget::Claude, home.path(), "medulla-babysit").exists());
}
