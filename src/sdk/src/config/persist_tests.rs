//! Unit tests for onboarding-state persistence ([`super::persist`]).
//!
//! Split out of the main config tests to keep both files under the repository's
//! 500-line ceiling.

use super::*;

#[test]
fn persists_the_welcome_flag_to_a_new_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nested").join("config.toml");

    super::persist_welcome_completed(&path, true).expect("persist should succeed");

    let text = std::fs::read_to_string(&path).expect("file should exist");
    let parsed: TuiConfig = toml::from_str(&text).expect("should reparse");
    assert!(parsed.onboarding.welcome_completed);
}

#[test]
fn persisting_the_welcome_flag_preserves_unrelated_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "stateDir = \"/tmp/keep-me\"\n\n[theme]\nprimary = \"#ff0000\"\n",
    )
    .expect("seed config");

    super::persist_welcome_completed(&path, true).expect("persist should succeed");

    let text = std::fs::read_to_string(&path).expect("read back");
    let parsed: TuiConfig = toml::from_str(&text).expect("should reparse");
    assert!(parsed.onboarding.welcome_completed);
    assert_eq!(parsed.state_dir, "/tmp/keep-me");
    assert_eq!(parsed.theme.primary.as_deref(), Some("#ff0000"));
}

#[test]
fn the_welcome_flag_can_be_cleared_to_replay_onboarding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    super::persist_welcome_completed(&path, true).expect("persist true");
    super::persist_welcome_completed(&path, false).expect("persist false");

    let text = std::fs::read_to_string(&path).expect("read back");
    let parsed: TuiConfig = toml::from_str(&text).expect("should reparse");
    assert!(!parsed.onboarding.welcome_completed);
}

#[test]
fn welcome_flag_defaults_to_false_when_absent() {
    let parsed: TuiConfig = toml::from_str("stateDir = \"/tmp/x\"\n").expect("parse");
    assert!(!parsed.onboarding.welcome_completed);
}

#[test]
fn persisting_over_an_unparseable_config_errors_rather_than_clobbering_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "this is not = = valid toml [[[").expect("seed");

    let err = super::persist_welcome_completed(&path, true)
        .expect_err("an unparseable config must not be silently overwritten");

    assert!(err.to_string().contains("Cannot parse"), "got: {err}");
    // The original bytes survive, so the user can fix them by hand.
    assert!(std::fs::read_to_string(&path)
        .expect("still readable")
        .contains("not = = valid"));
}

#[test]
fn persisting_under_a_file_masquerading_as_a_directory_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "i am a file").expect("seed");

    // `blocker` is a file, so treating it as a parent directory fails. Which
    // syscall reports it is platform-dependent: unix surfaces ENOTDIR from the
    // read (naming the full path), Windows fails at create_dir_all (naming the
    // parent). Assert only what must hold everywhere — a clean error naming the
    // offending path, never a panic.
    let target = blocker.join("config.toml");
    let err =
        super::persist_welcome_completed(&target, true).expect_err("cannot write under a file");

    let message = err.to_string();
    assert!(
        message.contains("Cannot read") || message.contains("Cannot create"),
        "got: {message}"
    );
    assert!(
        message.contains("blocker"),
        "should name the offending path: {message}"
    );
}

#[test]
fn persisting_merges_into_an_existing_onboarding_section() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[onboarding]\nwelcomeCompleted = false\nsomeFutureKey = \"keep me\"\n",
    )
    .expect("seed");

    super::persist_welcome_completed(&path, true).expect("persist");

    let text = std::fs::read_to_string(&path).expect("read back");
    assert!(text.contains("welcomeCompleted = true"));
    assert!(
        text.contains("keep me"),
        "unrelated onboarding keys must survive: {text}"
    );
}

#[test]
fn persisting_replaces_a_non_table_onboarding_value() {
    // A hand-edited config could set `onboarding` to a scalar; writing the flag
    // must still succeed rather than panicking on the unexpected shape.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "onboarding = \"nonsense\"\n").expect("seed");

    super::persist_welcome_completed(&path, true).expect("persist");

    let parsed: TuiConfig =
        toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("reparse");
    assert!(parsed.onboarding.welcome_completed);
}

#[test]
fn persist_setting_creates_and_merges_sections() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nested").join("config.toml");

    // Writing into a file that does not exist yet creates it and its parent.
    super::persist_setting(&path, "memory", "enabled", toml::Value::Boolean(true)).expect("write");
    // A second key in the same section merges rather than replacing.
    super::persist_setting(&path, "medulla", "maxPasses", toml::Value::Integer(7)).expect("write");

    let parsed: TuiConfig =
        toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("reparse");
    assert_eq!(parsed.memory.and_then(|m| m.enabled), Some(true));
    assert_eq!(parsed.medulla.max_passes, Some(7));
}

#[test]
fn persist_setting_preserves_unrelated_sections() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[theme]\nprimary = \"cyan\"\n\n[memory]\nworkspace = \"/w\"\n",
    )
    .expect("seed");

    super::persist_setting(&path, "memory", "enabled", toml::Value::Boolean(false)).expect("write");

    let parsed: TuiConfig =
        toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("reparse");
    assert_eq!(parsed.theme.primary.as_deref(), Some("cyan"));
    let memory = parsed.memory.expect("memory section");
    assert_eq!(memory.enabled, Some(false));
    assert_eq!(
        memory.workspace.as_deref(),
        Some("/w"),
        "sibling key survives"
    );
}

#[test]
fn clear_setting_removes_only_its_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[medulla]\nmaxPasses = 9\nmaxSteps = 40\n").expect("seed");

    super::clear_setting(&path, "medulla", "maxPasses").expect("clear");

    let parsed: TuiConfig =
        toml::from_str(&std::fs::read_to_string(&path).expect("read")).expect("reparse");
    assert_eq!(parsed.medulla.max_passes, None, "cleared back to unset");
    assert_eq!(parsed.medulla.max_steps, Some(40));
}

#[test]
fn clear_setting_on_a_missing_file_is_a_no_op() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("absent.toml");
    super::clear_setting(&path, "medulla", "maxPasses").expect("no-op");
    assert!(!path.exists(), "clearing must not create the file");
}
