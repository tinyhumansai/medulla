//! Unit tests for target/scope path resolution: where a skill or command file
//! lands for each harness, how a scope root resolves against the operator's
//! or Medulla's own home, and the argv a managed root contributes once it
//! holds skills.
//!
//! Kept apart from [`super::tests`], which exercises the install pipeline that
//! writes through these paths rather than the paths themselves.

use std::collections::HashMap;
use std::path::Path;

use tempfile::TempDir;

use crate::workflows::WorkflowSummary;

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

    let repo = Path::new("/work/repo");
    let root = scope_root(SkillScope::Managed, &env, repo);
    assert_eq!(root, managed_root(&env, repo));
    assert!(
        root.starts_with(home.path()),
        "the managed root belongs to Medulla, not the operator: {}",
        root.display()
    );

    // And it is per workspace, because the catalog it renders is: a store
    // discovered for a directory reads that directory's `.medulla/workflows`
    // too, so one shared root would let two projects overwrite and prune each
    // other's skills.
    assert_ne!(
        root,
        managed_root(&env, Path::new("/work/other")),
        "two workspaces must not share a managed root"
    );

    // Each harness gets its own directory, so the path handed to one harness
    // never exposes another's files. Laid out inside like a project root, which
    // is what makes --add-dir find it.
    let claude = managed_dir(SkillTarget::Claude, &env, repo);
    assert_eq!(claude, root.join("claude-skills"));
    assert_eq!(
        managed_dir(SkillTarget::Codex, &env, repo),
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
    let cwd = TempDir::new().unwrap();
    let cwd = cwd.path();

    // Nothing installed yet: the argv is untouched. Granting a session access to
    // a directory that holds nothing buys nothing and shows up in the operator's
    // transcript as an unexplained extra directory.
    assert!(spawn_args(provider, &env, cwd).is_empty());

    let root = managed_root(&env, cwd);
    let summary = summary("babysit", "Watch a PR.");
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
    let claude = managed_dir(SkillTarget::Claude, &env, cwd);
    assert!(skill_path(SkillTarget::Claude, &claude, "medulla-babysit").is_file());
    assert_eq!(
        spawn_args(provider, &env, cwd),
        vec!["--add-dir".to_string(), claude.display().to_string()],
        "Claude loads .claude/skills from an --add-dir directory; that flag is the \
         only spelling that does it"
    );

    // Codex has no equivalent flag, so it gets nothing rather than a flag its
    // CLI would reject.
    assert!(spawn_args(crate::protocol::HarnessProvider::Codex, &env, cwd).is_empty());
}

#[test]
fn default_targets_are_the_harnesses_already_present() {
    let home = TempDir::new().unwrap();
    assert!(default_targets(home.path()).is_empty());

    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::create_dir_all(home.path().join(".agents")).unwrap();
    assert_eq!(
        default_targets(home.path()),
        vec![SkillTarget::Claude, SkillTarget::Generic]
    );

    // Codex is detected by its config directory, which is where its config,
    // prompts, and credentials still live — its *skills* are the only thing
    // that moved to `.agents`.
    std::fs::create_dir_all(home.path().join(".codex")).unwrap();
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
