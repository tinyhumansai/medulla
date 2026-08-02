//! Legacy Git environment injection and escaping regressions.

use std::collections::HashMap;

/// Existing indexed and inline Git config remains intact around our hook path.
#[test]
fn inherited_git_config_entries_are_preserved() {
    let base = HashMap::from([
        ("GIT_CONFIG_COUNT".to_string(), "2".to_string()),
        ("GIT_CONFIG_KEY_0".to_string(), "user.email".to_string()),
        (
            "GIT_CONFIG_VALUE_0".to_string(),
            "ci@example.com".to_string(),
        ),
        ("GIT_CONFIG_KEY_1".to_string(), "safe.directory".to_string()),
        ("GIT_CONFIG_VALUE_1".to_string(), "*".to_string()),
        (
            "GIT_CONFIG_PARAMETERS".to_string(),
            "'user.name=CI User'".to_string(),
        ),
    ]);
    let env = super::super::attribution_env(true, &base);

    assert_eq!(env.get("GIT_CONFIG_COUNT"), Some(&"3".to_string()));
    assert_eq!(
        env.get("GIT_CONFIG_KEY_2"),
        Some(&"core.hooksPath".to_string())
    );
    assert!(!env.contains_key("GIT_CONFIG_KEY_0") && !env.contains_key("GIT_CONFIG_KEY_1"));
    assert_eq!(
        env.get("MEDULLA_GIT_CONFIG_BASE_COUNT"),
        Some(&"2".to_string())
    );
    assert_eq!(
        env.get("MEDULLA_GIT_CONFIG_BASE_PARAMETERS"),
        Some(&"'user.name=CI User'".to_string())
    );
    assert!(env["GIT_CONFIG_PARAMETERS"].starts_with("'user.name=CI User' "));
}

/// With no inherited Git config, the hook override occupies slot zero.
#[test]
fn a_clean_environment_puts_our_pair_at_slot_zero() {
    let env = super::super::attribution_env(true, &HashMap::new());
    assert_eq!(env.get("GIT_CONFIG_COUNT"), Some(&"1".to_string()));
    assert_eq!(
        env.get("GIT_CONFIG_KEY_0"),
        Some(&"core.hooksPath".to_string())
    );
    assert_eq!(
        env.get("MEDULLA_GIT_CONFIG_BASE_COUNT"),
        Some(&"0".to_string())
    );
    assert!(env["GIT_CONFIG_PARAMETERS"].contains("core.hooksPath="));
}

/// Legacy inline config remains parseable through apostrophes and whitespace.
#[test]
fn legacy_hook_path_escapes_apostrophes() {
    let path = "/tmp/Medulla agent's hooks";
    let parameter =
        super::super::prepare_commit_msg::legacy_config_parameter("core.hooksPath", path);
    let output = std::process::Command::new("git")
        .args(["config", "--get", "core.hooksPath"])
        .env_remove("GIT_CONFIG_COUNT")
        .env("GIT_CONFIG_PARAMETERS", parameter)
        .output()
        .expect("git reads legacy inline config");

    assert!(
        output.status.success(),
        "git rejected encoded path: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), path);
}

/// The shim restores legacy inline config while finding the original hook.
#[cfg(unix)]
#[test]
fn legacy_inline_hook_path_is_delegated_during_a_real_commit() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let original = root.path().join("original-hooks");
    let shim = root.path().join("medulla-hooks");
    let marker = root.path().join("original-ran");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&original).unwrap();
    std::fs::create_dir_all(&shim).unwrap();

    let original_hook = original.join("pre-commit");
    std::fs::write(
        &original_hook,
        format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&original_hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    let shim_hook = shim.join("pre-commit");
    std::fs::write(&shim_hook, super::super::prepare_commit_msg::HOOK_SHIM).unwrap();
    std::fs::set_permissions(&shim_hook, std::fs::Permissions::from_mode(0o700)).unwrap();

    let run = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(&["init", "-q"]);
    run(&["config", "user.name", "Medulla Test"]);
    run(&["config", "user.email", "medulla@example.test"]);
    std::fs::write(repo.join("change.txt"), "change\n").unwrap();
    run(&["add", "change.txt"]);

    let base = super::super::prepare_commit_msg::legacy_config_parameter(
        "core.hooksPath",
        &original.to_string_lossy(),
    );
    let injected = super::super::prepare_commit_msg::legacy_config_parameter(
        "core.hooksPath",
        &shim.to_string_lossy(),
    );
    let output = std::process::Command::new("git")
        .args(["commit", "-q", "-m", "exercise the shim"])
        .current_dir(&repo)
        .env("GIT_CONFIG_PARAMETERS", format!("{base} {injected}"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_0", &shim)
        .env("MEDULLA_GIT_CONFIG_BASE_COUNT", "0")
        .env("MEDULLA_GIT_CONFIG_BASE_PARAMETERS", &base)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(marker.exists(), "the inherited pre-commit hook did not run");
}

/// A malformed inherited count is treated as absent rather than guessed.
#[test]
fn a_malformed_inherited_count_falls_back_to_slot_zero() {
    let base = HashMap::from([("GIT_CONFIG_COUNT".to_string(), "not-a-number".to_string())]);
    let env = super::super::attribution_env(true, &base);
    assert_eq!(env.get("GIT_CONFIG_COUNT"), Some(&"1".to_string()));
    assert_eq!(
        env.get("GIT_CONFIG_KEY_0"),
        Some(&"core.hooksPath".to_string())
    );
}
