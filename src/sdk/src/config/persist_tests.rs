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
