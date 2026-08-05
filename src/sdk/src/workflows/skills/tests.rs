//! Unit tests for the install/sync/uninstall pipeline and the managed-file
//! discipline that guards it.
//!
//! The marker discipline gets the most attention here on purpose: it is the
//! only thing standing between this feature and clobbering a hand-written file
//! in someone's `~/.claude`, and unlike the rendering, a regression in it is
//! not visible by reading the output. Rendering itself is covered in
//! [`super::render_tests`], and target/scope path resolution in
//! [`super::targets_tests`].

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

/// A file the previous release installed carries its marker as an HTML comment
/// above the frontmatter. It is still ours: it must be recognised, must be
/// rewritten into the readable layout, and must never be reported as a
/// collision against the very skill that wrote it.
#[test]
fn a_skill_the_old_layout_installed_is_adopted_and_rewritten() {
    let home = TempDir::new().unwrap();
    let summary = summary("babysit", "Watch a PR.");
    let skill = render(&summary);
    let path = skill_path(
        SkillTarget::Claude,
        home.path(),
        &crate::workflows::skills::slug_for("babysit"),
    );
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // The old file: the same rendered content, marker on line one.
    let content = skill.body.replacen(
        &format!("# medulla:managed workflow=babysit rev={}\n", skill.rev),
        "",
        1,
    );
    std::fs::write(
        &path,
        format!("<!-- medulla:managed workflow=babysit rev=deadbeef -->\n{content}"),
    )
    .unwrap();

    assert_eq!(
        parse_marker(&std::fs::read_to_string(&path).unwrap()).map(|(id, _)| id),
        Some("babysit".to_string()),
        "the legacy marker must still identify the file as ours"
    );

    let report = install(&[summary], &opts(home.path(), vec![SkillTarget::Claude])).unwrap();

    assert_eq!(report.files[0].action, FileAction::Updated);
    assert!(!report.has_collisions());
    assert!(std::fs::read_to_string(&path).unwrap().starts_with("---\n"));
}

/// A hand-written skill can legitimately mention `# medulla:managed` in its own
/// prose — for example a document *about* this feature. If its frontmatter
/// never closes, that line is never inside the frontmatter at all, so it must
/// not be read as a marker: doing so would let Medulla adopt, and later prune,
/// a file it never wrote.
#[test]
fn an_unclosed_frontmatter_is_never_scanned_for_a_marker() {
    let unclosed = "---\n# medulla:managed workflow=babysit rev=abc\nno closing delimiter below\n";
    assert_eq!(parse_marker(unclosed), None);
}

/// `seal` always splices the marker onto the line directly after the opening
/// `---`, so that is the only slot a real file of ours ever has it in. A
/// hand-written skill can legitimately mention `# medulla:managed` deeper in
/// its own (otherwise valid, closed) frontmatter — a migration note, or
/// documentation about this feature — and that must not be read as a marker
/// either, for the same reason an unclosed block is rejected: it would let
/// Medulla adopt, and later overwrite or prune, a file it never wrote.
#[test]
fn a_marker_deeper_in_a_closed_frontmatter_block_is_not_recognised() {
    let deeper =
        "---\nname: not-a-real-skill\n# medulla:managed workflow=babysit rev=abc\n---\nbody\n";
    assert_eq!(parse_marker(deeper), None);
}

#[test]
fn codex_and_generic_share_one_directory_and_are_visited_once() {
    // Both resolve to `.agents/skills`. Naming both must not write the file
    // twice, report it twice, or list it under two harness names.
    let home = TempDir::new().unwrap();
    let options = opts(
        home.path(),
        vec![
            SkillTarget::Codex,
            SkillTarget::Generic,
            SkillTarget::Claude,
        ],
    );
    let workflows = vec![summary("babysit", "Watch a PR.")];

    let report = install(&workflows, &options).unwrap();
    assert_eq!(report.count(FileAction::Created), 2, "{report:?}");
    assert!(!report.has_collisions(), "{report:?}");
    assert_eq!(installed(&options).unwrap().len(), 2);

    // The first target named keeps the shared directory.
    let shared = installed(&options)
        .unwrap()
        .into_iter()
        .find(|skill| skill.path.starts_with(home.path().join(".agents")))
        .expect("the shared .agents skill is installed");
    assert_eq!(shared.target, SkillTarget::Codex);
}

#[test]
fn an_install_retires_the_skill_an_older_release_left_in_codexs_own_root() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Codex]);
    let workflows = vec![summary("babysit", "Watch a PR.")];

    // What a previous version of this command wrote: a *managed* file under
    // `.codex/skills`. Codex still scans that root, so leaving it there would
    // make `$medulla-babysit` resolve to two skills and silently stop working.
    let legacy = home
        .path()
        .join(".codex")
        .join("skills")
        .join("medulla-babysit")
        .join("SKILL.md");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let rendered = render(&workflows[0]);
    fs::write(&legacy, &rendered.body).unwrap();

    // A hand-written neighbour in the same root must survive.
    let theirs = home
        .path()
        .join(".codex")
        .join("skills")
        .join("mine")
        .join("SKILL.md");
    fs::create_dir_all(theirs.parent().unwrap()).unwrap();
    fs::write(&theirs, "mine, not yours").unwrap();

    let report = install(&workflows, &options).unwrap();
    assert_eq!(report.count(FileAction::Removed), 1, "{report:?}");
    assert!(!legacy.exists(), "the legacy managed copy is retired");
    assert!(skill_path(SkillTarget::Codex, home.path(), &rendered.slug).exists());
    assert_eq!(fs::read_to_string(&theirs).unwrap(), "mine, not yours");

    // Nothing left to retire on the next run.
    let second = install(&workflows, &options).unwrap();
    assert_eq!(second.count(FileAction::Removed), 0, "{second:?}");
}

#[test]
fn install_is_idempotent() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);
    let workflows = vec![summary("babysit", "Watch a PR.")];

    let first = install(&workflows, &options).unwrap();
    assert_eq!(first.count(FileAction::Created), 1);
    let path = skill_path(SkillTarget::Claude, home.path(), "medulla-babysit");
    let written = fs::read_to_string(&path).unwrap();

    let second = install(&workflows, &options).unwrap();
    assert_eq!(second.count(FileAction::Unchanged), 1);
    assert_eq!(second.count(FileAction::Created), 0);
    assert_eq!(fs::read_to_string(&path).unwrap(), written);
    assert!(!second.has_collisions());
}

#[test]
fn a_changed_workflow_updates_its_managed_file() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);
    install(&[summary("babysit", "Watch a PR.")], &options).unwrap();

    let report = install(&[summary("babysit", "Watch a PR closely.")], &options).unwrap();

    assert_eq!(report.count(FileAction::Updated), 1);
    let path = skill_path(SkillTarget::Claude, home.path(), "medulla-babysit");
    assert!(fs::read_to_string(path).unwrap().contains("closely"));
}

#[test]
fn an_unmarked_collision_is_skipped_with_bytes_untouched() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);
    let path = skill_path(SkillTarget::Claude, home.path(), "medulla-babysit");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "hand written, do not touch\n").unwrap();

    let report = install(&[summary("babysit", "Watch a PR.")], &options).unwrap();

    assert!(report.has_collisions());
    assert_eq!(report.count(FileAction::SkippedUnmanaged), 1);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "hand written, do not touch\n"
    );
}

#[test]
fn a_marker_for_another_workflow_is_also_left_alone() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);
    let path = skill_path(SkillTarget::Claude, home.path(), "medulla-babysit");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let squatter = "<!-- medulla:managed workflow=other rev=abc -->\nnot yours\n";
    fs::write(&path, squatter).unwrap();

    let report = install(&[summary("babysit", "Watch a PR.")], &options).unwrap();

    // Ours, but another workflow's: a collision rather than an unmanaged file,
    // because "someone else wrote this" would send the operator hunting for a
    // third party who does not exist.
    assert_eq!(report.count(FileAction::SlugCollision), 1);
    assert_eq!(report.count(FileAction::SkippedUnmanaged), 0);
    assert!(report.has_collisions());
    assert_eq!(fs::read_to_string(&path).unwrap(), squatter);
}

#[test]
fn dry_run_reports_without_writing() {
    let home = TempDir::new().unwrap();
    let mut options = opts(home.path(), vec![SkillTarget::Claude]);
    options.dry_run = true;

    let report = install(&[summary("babysit", "Watch a PR.")], &options).unwrap();

    assert_eq!(report.count(FileAction::Created), 1);
    assert!(!skill_path(SkillTarget::Claude, home.path(), "medulla-babysit").exists());
    assert!(!home.path().join(".claude").exists());
}

#[test]
fn a_disabled_workflow_gets_nothing() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);
    let mut disabled = summary("babysit", "Watch a PR.");
    disabled.enabled = false;

    let report = install(&[disabled], &options).unwrap();

    assert!(report.files.is_empty());
    assert!(!skill_path(SkillTarget::Claude, home.path(), "medulla-babysit").exists());
}

#[test]
fn commands_are_written_only_when_asked_and_only_where_they_exist() {
    let home = TempDir::new().unwrap();
    let mut options = opts(
        home.path(),
        vec![
            SkillTarget::Claude,
            SkillTarget::Codex,
            SkillTarget::Generic,
        ],
    );
    options.with_commands = true;

    install(&[summary("babysit", "Watch a PR.")], &options).unwrap();

    assert!(home
        .path()
        .join(".claude/commands/medulla-babysit.md")
        .is_file());
    assert!(home
        .path()
        .join(".codex/prompts/medulla-babysit.md")
        .is_file());
    assert!(!home.path().join(".medulla/prompts").exists());
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

#[test]
fn uninstall_removes_only_the_named_workflows_files() {
    let home = TempDir::new().unwrap();
    let mut options = opts(home.path(), vec![SkillTarget::Claude]);
    options.with_commands = true;
    install(
        &[
            summary("babysit", "Watch a PR."),
            summary("audit", "Audit a repo."),
        ],
        &options,
    )
    .unwrap();

    let report = uninstall(&["babysit".to_string()], &options).unwrap();

    // Skill and command file both go.
    assert_eq!(report.count(FileAction::Removed), 2);
    assert!(!skill_path(SkillTarget::Claude, home.path(), "medulla-babysit").exists());
    assert!(!home
        .path()
        .join(".claude/commands/medulla-babysit.md")
        .exists());
    assert!(skill_path(SkillTarget::Claude, home.path(), "medulla-audit").is_file());

    // Repeating it is a no-op rather than an error.
    assert!(uninstall(&["babysit".to_string()], &options)
        .unwrap()
        .files
        .is_empty());
}

#[test]
fn installed_lists_marked_skills_only() {
    let home = TempDir::new().unwrap();
    let mut options = opts(home.path(), vec![SkillTarget::Claude]);
    options.with_commands = true;
    let workflows = vec![summary("babysit", "Watch a PR.")];
    install(&workflows, &options).unwrap();

    let stray = home.path().join(".claude/skills/handwritten/SKILL.md");
    fs::create_dir_all(stray.parent().unwrap()).unwrap();
    fs::write(&stray, "mine\n").unwrap();

    let found = installed(&options).unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].workflow_id, "babysit");
    assert_eq!(found[0].slug, "medulla-babysit");
    assert_eq!(found[0].target, SkillTarget::Claude);
    assert_eq!(found[0].rev, render(&workflows[0]).rev);
}
