//! Unit tests for skill rendering, target layout, and the managed-file
//! discipline.
//!
//! The marker discipline gets the most attention here on purpose: it is the
//! only thing standing between this feature and clobbering a hand-written file
//! in someone's `~/.claude`, and unlike the rendering, a regression in it is
//! not visible by reading the output.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::json;
use tempfile::TempDir;

use crate::workflows::{InputType, WorkflowInput, WorkflowSummary};

use super::render::parse_marker;
use super::*;

/// A listing view with the fields skill rendering actually reads.
fn summary(id: &str, description: &str, inputs: Vec<WorkflowInput>) -> WorkflowSummary {
    WorkflowSummary {
        id: id.to_string(),
        name: id.to_string(),
        description: description.to_string(),
        enabled: true,
        node_count: 3,
        trigger_kind: Some("manual".to_string()),
        inputs,
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
fn slug_prefixes_and_sanitises() {
    assert_eq!(slug_for("babysit"), "medulla-babysit");
    assert_eq!(slug_for("Review PR"), "medulla-review-pr");
    assert_eq!(slug_for("a//b"), "medulla-a-b");
    assert_eq!(slug_for("!!!"), "medulla-workflow");
}

#[test]
fn zero_input_skill_states_an_empty_inputs_object() {
    let skill = render(&summary("babysit", "Watch a pull request.", vec![]));

    assert_eq!(skill.slug, "medulla-babysit");
    // The frontmatter opens the file and the marker is the first thing inside
    // it. A harness only parses frontmatter that starts on line 1: with the
    // marker above it, the description — the one field a request is matched
    // against — is not read at all.
    assert!(
        skill
            .body
            .starts_with("---\n# medulla:managed workflow=babysit rev="),
        "{}",
        skill.body
    );
    assert!(skill.body.contains("name: medulla-babysit"));
    assert!(skill.body.contains("This workflow takes no inputs"));
    assert!(skill.body.contains("mcp__medulla__workflow_run"));
    assert!(skill.body.contains("\"inputs\": {}"));
    // The fallback keeps the skill useful with no server attached.
    assert!(skill
        .body
        .contains("medulla workflow run babysit --inputs '{}'"));
    assert!(skill.body.contains("medulla skills install --with-mcp"));
    // Blocking behaviour is stated, because it is the surprising part.
    assert!(skill.body.contains("blocks until the run finishes"));
}

#[test]
fn description_carries_the_workflow_words_and_a_trigger_clause() {
    let skill = render(&summary("babysit", "Watch a pull request", vec![]));
    assert_eq!(
        skill.description,
        "Watch a pull request. Use when the operator asks to run the Medulla \
         \"babysit\" workflow, or describes the work it does."
    );
}

#[test]
fn description_is_non_empty_when_the_workflow_has_none() {
    let skill = render(&summary("babysit", "   ", vec![]));
    assert_eq!(
        skill.description,
        "Use when the operator asks to run the Medulla \"babysit\" workflow, or \
         describes the work it does."
    );
    // Frontmatter carries it as a double-quoted scalar, so the workflow name's
    // own quotes are escaped rather than ending the string early.
    assert!(skill.body.contains(
        "description: \"Use when the operator asks to run the Medulla \\\"babysit\\\" workflow, \
         or describes the work it does.\"\n"
    ));
}

#[test]
fn optional_inputs_render_their_defaults_in_the_example() {
    let inputs = vec![
        WorkflowInput::new("repo", InputType::String)
            .with_default(json!("current"))
            .with_description("Repository to inspect"),
        WorkflowInput::new("deep", InputType::Boolean),
    ];
    let skill = render(&summary("audit", "Audit a repo.", inputs));

    assert!(skill
        .body
        .contains("| `repo` | string | no | Repository to inspect | `\"current\"` |"));
    assert!(skill.body.contains("| `deep` | boolean | no | — | — |"));
    // The default is shown as a runnable value; the undeclared one as a
    // type-shaped placeholder.
    assert!(skill.body.contains("\"repo\": \"current\""));
    assert!(skill.body.contains("\"deep\": false"));
}

#[test]
fn required_inputs_are_marked_and_placeheld() {
    let inputs = vec![
        WorkflowInput::new("pr", InputType::Number)
            .required()
            .with_description("The pull request number"),
        WorkflowInput::new("payload", InputType::Json).required(),
    ];
    let skill = render(&summary("babysit", "Watch a PR.", inputs.clone()));

    assert!(skill
        .body
        .contains("| `pr` | number | yes | The pull request number | — |"));
    assert!(skill.body.contains("| `payload` | json | yes | — | — |"));
    assert!(skill.body.contains("\"pr\": 0"));
    assert!(skill.body.contains("do not invent"));

    let command = render_command(&skill, &summary("babysit", "Watch a PR.", inputs));
    assert!(command.contains("argument-hint: \"<pr> <payload>\""));
    assert!(command.contains("$ARGUMENTS"));
    // A command file is frontmatter-led too, so its marker sits in the same
    // place for the same reason.
    assert!(
        command.starts_with("---\n# medulla:managed workflow=babysit rev="),
        "{command}"
    );
}

#[test]
fn rev_changes_only_when_the_body_does() {
    let first = render(&summary("babysit", "Watch a PR.", vec![]));
    let same = render(&summary("babysit", "Watch a PR.", vec![]));
    let changed = render(&summary("babysit", "Watch a PR closely.", vec![]));

    assert_eq!(first.rev, same.rev);
    assert_eq!(first.body, same.body);
    assert_ne!(first.rev, changed.rev);

    // The rev fingerprints the generated content, and appears only on the
    // marker line itself — so reading it back off the file cannot pick up a
    // hash the content happens to quote.
    assert!(first.body.contains(&format!("rev={}", first.rev)));
    let elsewhere: String = first
        .body
        .lines()
        .filter(|line| !line.starts_with("# medulla:managed "))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!elsewhere.contains(&first.rev));
}

/// A file the previous release installed carries its marker as an HTML comment
/// above the frontmatter. It is still ours: it must be recognised, must be
/// rewritten into the readable layout, and must never be reported as a
/// collision against the very skill that wrote it.
#[test]
fn a_skill_the_old_layout_installed_is_adopted_and_rewritten() {
    let home = TempDir::new().unwrap();
    let summary = summary("babysit", "Watch a PR.", Vec::new());
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

#[test]
fn every_target_has_its_documented_layout() {
    let root = Path::new("/root");

    assert_eq!(
        skill_path(SkillTarget::Claude, root, "medulla-x"),
        Path::new("/root/.claude/skills/medulla-x/SKILL.md")
    );
    // Codex reads `.agents/skills`, anchored to the real home rather than to
    // CODEX_HOME. Its own `.codex/skills` is scanned too, but upstream calls
    // that one deprecated, and an operator with CODEX_HOME set would never see
    // a file we wrote there.
    assert_eq!(
        skill_path(SkillTarget::Codex, root, "medulla-x"),
        Path::new("/root/.agents/skills/medulla-x/SKILL.md")
    );
    // The generic target shares that location: the Agent Skills convention is
    // the one thing unverified harnesses have in common.
    assert_eq!(
        skill_path(SkillTarget::Generic, root, "medulla-x"),
        Path::new("/root/.agents/skills/medulla-x/SKILL.md")
    );

    assert_eq!(
        command_path(SkillTarget::Claude, root, "medulla-x").unwrap(),
        Path::new("/root/.claude/commands/medulla-x.md")
    );
    assert_eq!(
        command_path(SkillTarget::Codex, root, "medulla-x").unwrap(),
        Path::new("/root/.codex/prompts/medulla-x.md")
    );
    assert!(command_path(SkillTarget::Generic, root, "medulla-x").is_none());
}

#[test]
fn scope_root_reads_home_for_user_and_cwd_for_project() {
    let mut env = HashMap::new();
    env.insert("HOME".to_string(), "/home/op".to_string());
    let cwd = Path::new("/work/repo");

    assert_eq!(
        scope_root(SkillScope::User, &env, cwd),
        Path::new("/home/op")
    );
    assert_eq!(
        scope_root(SkillScope::Project, &env, cwd),
        Path::new("/work/repo")
    );

    // An unset or blank HOME falls back rather than panicking.
    env.insert("HOME".to_string(), "  ".to_string());
    assert_eq!(scope_root(SkillScope::User, &env, cwd), cwd);
    assert_eq!(scope_root(SkillScope::User, &HashMap::new(), cwd), cwd);
}

#[test]
fn the_managed_scope_resolves_under_the_medulla_home_not_the_operators() {
    let home = TempDir::new().unwrap();
    let mut env = HashMap::new();
    env.insert(
        "MEDULLA_HOME".to_string(),
        home.path().display().to_string(),
    );
    env.insert("HOME".to_string(), "/home/op".to_string());

    let root = scope_root(SkillScope::Managed, &env, Path::new("/work/repo"));
    assert_eq!(root, managed_root(&env));
    assert!(
        root.starts_with(home.path()),
        "the managed root belongs to Medulla, not the operator: {}",
        root.display()
    );

    // Each harness gets its own directory, so the path handed to one harness
    // never exposes another's files. Laid out inside like a project root, which
    // is what makes --add-dir find it.
    let claude = managed_dir(SkillTarget::Claude, &env);
    assert_eq!(claude, root.join("claude-skills"));
    assert_eq!(
        managed_dir(SkillTarget::Codex, &env),
        root.join("codex-skills")
    );
    assert_eq!(
        skill_path(SkillTarget::Claude, &claude, "medulla-babysit"),
        claude
            .join(".claude")
            .join("skills")
            .join("medulla-babysit")
            .join("SKILL.md")
    );
}

#[test]
fn a_spawned_claude_is_pointed_at_the_managed_root_only_once_it_holds_skills() {
    let home = TempDir::new().unwrap();
    let mut env = HashMap::new();
    env.insert(
        "MEDULLA_HOME".to_string(),
        home.path().display().to_string(),
    );
    let provider = crate::protocol::HarnessProvider::Claude;

    // Nothing installed yet: the argv is untouched. Granting a session access to
    // a directory that holds nothing buys nothing and shows up in the operator's
    // transcript as an unexplained extra directory.
    assert!(spawn_args(provider, &env).is_empty());

    let root = managed_root(&env);
    let summary = summary("babysit", "Watch a PR.", Vec::new());
    install(
        &[summary],
        &InstallOptions {
            targets: vec![SkillTarget::Claude],
            scope: SkillScope::Managed,
            root: root.clone(),
            with_commands: false,
            dry_run: false,
        },
    )
    .unwrap();

    // The install lands in the harness's own directory, and that directory —
    // not the shared root — is what the session is given.
    let claude = managed_dir(SkillTarget::Claude, &env);
    assert!(skill_path(SkillTarget::Claude, &claude, "medulla-babysit").is_file());
    assert_eq!(
        spawn_args(provider, &env),
        vec!["--add-dir".to_string(), claude.display().to_string()],
        "Claude loads .claude/skills from an --add-dir directory; that flag is the \
         only spelling that does it"
    );

    // Codex has no equivalent flag, so it gets nothing rather than a flag its
    // CLI would reject.
    assert!(spawn_args(crate::protocol::HarnessProvider::Codex, &env).is_empty());
}

#[test]
fn default_targets_are_the_harnesses_already_present() {
    let home = TempDir::new().unwrap();
    assert!(default_targets(home.path()).is_empty());

    fs::create_dir_all(home.path().join(".claude")).unwrap();
    fs::create_dir_all(home.path().join(".agents")).unwrap();
    assert_eq!(
        default_targets(home.path()),
        vec![SkillTarget::Claude, SkillTarget::Generic]
    );

    // Codex is detected by its config directory, which is where its config,
    // prompts, and credentials still live — its *skills* are the only thing
    // that moved to `.agents`.
    fs::create_dir_all(home.path().join(".codex")).unwrap();
    assert_eq!(
        default_targets(home.path()),
        vec![
            SkillTarget::Claude,
            SkillTarget::Codex,
            SkillTarget::Generic
        ]
    );
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
    let workflows = vec![summary("babysit", "Watch a PR.", vec![])];

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
    let workflows = vec![summary("babysit", "Watch a PR.", vec![])];

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
    let workflows = vec![summary("babysit", "Watch a PR.", vec![])];

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
    install(&[summary("babysit", "Watch a PR.", vec![])], &options).unwrap();

    let report = install(
        &[summary("babysit", "Watch a PR closely.", vec![])],
        &options,
    )
    .unwrap();

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

    let report = install(&[summary("babysit", "Watch a PR.", vec![])], &options).unwrap();

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

    let report = install(&[summary("babysit", "Watch a PR.", vec![])], &options).unwrap();

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

    let report = install(&[summary("babysit", "Watch a PR.", vec![])], &options).unwrap();

    assert_eq!(report.count(FileAction::Created), 1);
    assert!(!skill_path(SkillTarget::Claude, home.path(), "medulla-babysit").exists());
    assert!(!home.path().join(".claude").exists());
}

#[test]
fn a_disabled_workflow_gets_nothing() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);
    let mut disabled = summary("babysit", "Watch a PR.", vec![]);
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

    install(&[summary("babysit", "Watch a PR.", vec![])], &options).unwrap();

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
    let kept = summary("babysit", "Watch a PR.", vec![]);
    let orphan = summary("audit", "Audit a repo.", vec![]);
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
    let kept = summary("babysit", "Watch a PR.", vec![]);
    install(
        &[kept.clone(), summary("audit", "Audit.", vec![])],
        &options,
    )
    .unwrap();

    let report = sync(&[kept], &options, false).unwrap();

    assert_eq!(report.count(FileAction::Removed), 0);
    assert!(skill_path(SkillTarget::Claude, home.path(), "medulla-audit").is_file());
}

#[test]
fn sync_prunes_a_workflow_that_has_since_been_disabled() {
    let home = TempDir::new().unwrap();
    let options = opts(home.path(), vec![SkillTarget::Claude]);
    install(&[summary("babysit", "Watch a PR.", vec![])], &options).unwrap();

    let mut disabled = summary("babysit", "Watch a PR.", vec![]);
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
            summary("babysit", "Watch a PR.", vec![]),
            summary("audit", "Audit a repo.", vec![]),
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
    let workflows = vec![summary("babysit", "Watch a PR.", vec![])];
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

#[test]
fn target_and_scope_parse_round_trip() {
    for target in SkillTarget::ALL {
        assert_eq!(SkillTarget::parse(target.as_str()).unwrap(), target);
    }
    assert_eq!(SkillTarget::parse(" CLAUDE ").unwrap(), SkillTarget::Claude);
    assert!(SkillTarget::parse("cursor").is_err());

    assert_eq!(SkillScope::parse("User").unwrap(), SkillScope::User);
    assert_eq!(SkillScope::parse("project").unwrap(), SkillScope::Project);
    assert!(SkillScope::parse("global").is_err());
}
