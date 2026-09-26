//! Tests for startup retirement of legacy ambient workflow skills.

use std::collections::HashMap;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
use serde_json::json;

use crate::startup_skills::{reconcile, reconcile_for_test};

/// A small valid workflow document, sufficient for the generated skill pass.
#[cfg(unix)]
fn workflow() -> String {
    json!({
        "id": "startup-check",
        "name": "Startup check",
        "description": "Verify startup integration",
        "nodes": [
            { "id": "trigger", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "agent", "kind": "agent", "name": "work",
              "config": { "prompt": "check it" } }
        ],
        "edges": [{ "from_node": "trigger", "to_node": "agent" }]
    })
    .to_string()
}

#[test]
#[cfg(unix)]
fn startup_retires_generated_user_skills_instead_of_exposing_them_globally() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let cwd = fixture.path().join("project");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(cwd.join(".medulla/workflows")).unwrap();
    fs::write(
        cwd.join(".medulla/workflows/startup-check.json"),
        workflow(),
    )
    .unwrap();

    let claude_args = fixture.path().join("claude-args");
    let claude = fixture.path().join("claude");
    fs::write(
        &claude,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\n",
            claude_args.display(),
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&claude).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&claude, permissions).unwrap();

    let medulla = fixture.path().join("medulla");
    fs::write(&medulla, "test binary placeholder").unwrap();
    let env = HashMap::from([
        ("HOME".to_string(), home.display().to_string()),
        (
            "MEDULLA_HOME".to_string(),
            fixture.path().join("medulla-home").display().to_string(),
        ),
    ]);

    let loaded = medulla::workflows::store::discover(&env, &cwd).load();
    let workflows: Vec<_> = loaded
        .workflows
        .iter()
        .map(medulla::workflows::WorkflowRecord::summary)
        .collect();
    medulla::workflows::skills::sync(
        &workflows,
        &medulla::workflows::skills::InstallOptions {
            targets: vec![
                medulla::workflows::skills::SkillTarget::Claude,
                medulla::workflows::skills::SkillTarget::Codex,
            ],
            scope: medulla::workflows::skills::SkillScope::User,
            root: home.clone(),
            with_commands: false,
            dry_run: false,
        },
        true,
    )
    .unwrap();
    assert!(home
        .join(".claude/skills/medulla-startup-check/SKILL.md")
        .is_file());
    assert!(home
        .join(".agents/skills/medulla-startup-check/SKILL.md")
        .is_file());

    let report = reconcile_for_test(&env, &cwd, &medulla, &claude);

    assert!(report.warnings.is_empty(), "{report:?}");
    assert!(report.notice.is_some(), "{report:?}");
    assert!(!home
        .join(".claude/skills/medulla-startup-check/SKILL.md")
        .is_file());
    assert!(!home
        .join(".agents/skills/medulla-startup-check/SKILL.md")
        .is_file());
    assert!(!home.join(".codex/config.toml").exists());
    assert!(
        !claude_args.exists(),
        "startup no longer edits Claude's registry"
    );

    let again = reconcile_for_test(&env, &cwd, &medulla, &claude);
    assert!(again.warnings.is_empty(), "{again:?}");
    assert_eq!(again.notice, None);
}

#[test]
fn startup_does_nothing_when_no_supported_harness_is_installed() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let cwd = fixture.path().join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    let env = HashMap::from([("HOME".to_string(), home.display().to_string())]);

    let report = reconcile_for_test(
        &env,
        &cwd,
        &fixture.path().join("medulla"),
        &fixture.path().join("missing-claude"),
    );

    assert_eq!(report.notice, None);
    assert!(report.warnings.is_empty());
    assert!(!home.join(".agents").exists());
}

#[test]
fn scratch_medulla_home_never_changes_the_real_harness_home() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let cwd = fixture.path().join("project");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    let env = HashMap::from([
        ("HOME".to_string(), home.display().to_string()),
        (
            "MEDULLA_HOME".to_string(),
            fixture.path().join("scratch-medulla").display().to_string(),
        ),
    ]);

    let report = reconcile(&env, &cwd);

    assert_eq!(report.notice, None);
    assert!(report.warnings.is_empty());
    assert!(!home.join(".codex/config.toml").exists());
    assert!(!home.join(".claude/skills").exists());
    assert!(!home.join(".agents").exists());
}

#[test]
fn dev_medulla_home_never_changes_the_real_harness_home() {
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    let cwd = fixture.path().join("project");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    let env = HashMap::from([
        ("HOME".to_string(), home.display().to_string()),
        ("MEDULLA_DEV".to_string(), "true".to_string()),
    ]);

    let report = reconcile(&env, &cwd);

    assert_eq!(report.notice, None);
    assert!(report.warnings.is_empty());
    assert!(!home.join(".codex/config.toml").exists());
    assert!(!home.join(".claude/skills").exists());
    assert!(!home.join(".agents").exists());
}
