//! Focused tests for inherited and legacy Git configuration passed to hooks.

use std::collections::HashMap;

/// `GIT_CONFIG_COUNT` is the number of pairs Git reads, so our entry must be
/// appended to whatever the parent set.
#[cfg(unix)]
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
    let env = super::attribution_env(true, &base);

    assert_eq!(env.get("GIT_CONFIG_COUNT"), Some(&"3".to_string()));
    assert_eq!(
        env.get("GIT_CONFIG_KEY_2"),
        Some(&"core.hooksPath".to_string())
    );
    assert!(
        !env.contains_key("GIT_CONFIG_KEY_0") && !env.contains_key("GIT_CONFIG_KEY_1"),
        "inherited slots must be left alone: {env:?}"
    );
    assert_eq!(
        env.get("MEDULLA_GIT_CONFIG_BASE_COUNT"),
        Some(&"2".to_string())
    );
    assert_eq!(
        env.get("MEDULLA_GIT_CONFIG_BASE_PARAMETERS"),
        Some(&"'user.name=CI User'".to_string())
    );
    assert!(
        env["GIT_CONFIG_PARAMETERS"].starts_with("'user.name=CI User' "),
        "legacy inline config must be preserved: {env:?}"
    );
}

/// With no inherited Git config, the hook-path pair lands at slot zero.
#[cfg(unix)]
#[test]
fn a_clean_environment_puts_our_pair_at_slot_zero() {
    let env = super::attribution_env(true, &HashMap::new());
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

/// Legacy inline config remains parseable when a path contains punctuation.
#[cfg(unix)]
#[test]
fn legacy_hook_path_escapes_apostrophes() {
    let path = "/tmp/Medulla agent's hooks";
    let parameter = super::prepare_commit_msg::legacy_config_parameter("core.hooksPath", path);
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

/// A malformed inherited count falls back to the first Git config slot.
#[cfg(unix)]
#[test]
fn a_malformed_inherited_count_falls_back_to_slot_zero() {
    let base = HashMap::from([("GIT_CONFIG_COUNT".to_string(), "not-a-number".to_string())]);
    let env = super::attribution_env(true, &base);
    assert_eq!(env.get("GIT_CONFIG_COUNT"), Some(&"1".to_string()));
    assert_eq!(
        env.get("GIT_CONFIG_KEY_0"),
        Some(&"core.hooksPath".to_string())
    );
}
