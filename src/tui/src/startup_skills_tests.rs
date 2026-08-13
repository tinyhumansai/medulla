//! Tests for startup reconciliation of user skills and MCP registrations.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use serde_json::json;

use crate::startup_skills::{reconcile, reconcile_for_test};

/// A small valid workflow document, sufficient for the generated skill pass.
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
fn startup_syncs_both_harnesses_and_registers_their_mcp_server() {
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

    let claude_state = fixture.path().join("claude-registered");
    let claude_args = fixture.path().join("claude-args");
    let claude = fixture.path().join("claude");
    fs::write(
        &claude,
        format!(
            "#!/bin/sh\nif [ -f '{}' ]; then echo 'MCP server medulla already exists in user config' >&2; exit 1; fi\nprintf '%s\\n' \"$*\" > '{}'\ntouch '{}'\n",
            claude_state.display(),
            claude_args.display(),
            claude_state.display(),
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

    let report = reconcile_for_test(&env, &cwd, &medulla, &claude);

    assert!(report.warnings.is_empty(), "{report:?}");
    assert!(report.notice.is_some(), "{report:?}");
    assert!(home
        .join(".claude/skills/medulla-startup-check/SKILL.md")
        .is_file());
    assert!(home
        .join(".agents/skills/medulla-startup-check/SKILL.md")
        .is_file());
    let codex = fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    assert!(codex.contains("[mcp_servers.medulla]"), "{codex}");
    assert!(codex.contains(&medulla.display().to_string()), "{codex}");
    let args = fs::read_to_string(claude_args).unwrap();
    assert_eq!(
        args.trim(),
        format!(
            "mcp add --scope user medulla --env MEDULLA_WORKFLOW_TOOLS=run -- {} mcp",
            medulla.display()
        )
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
    assert!(!home.join(".agents").exists());
}
