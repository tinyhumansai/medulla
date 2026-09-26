//! Unit tests for the legacy-credential sweep: both candidate locations, the
//! dedicated [`LEGACY_CONFIG_DIR_OVERRIDE`] that makes the OS-config-directory
//! one testable on every platform without repurposing `XDG_CONFIG_HOME` (a
//! variable a real install can have set) or touching the real process
//! environment, and best-effort read/remove behavior.

use std::collections::HashMap;

use super::*;

/// No override in the injected map falls back to [`dirs::config_dir`] rather
/// than silently dropping the OS-config-directory candidate — a caller that
/// never sets it (every real production caller) still gets the second
/// location a real install can have a legacy credential in.
#[test]
fn without_an_override_the_real_os_config_directory_is_still_a_candidate() {
    let env = HashMap::new();
    let expected = dirs::config_dir().map(|d| d.join("medulla").join("credentials.json"));

    assert_eq!(config_dir_location(&env), expected);
}

/// The whole point of the override: a caller — in practice, a test — can
/// redirect the OS-config-directory candidate to a scratch path without
/// touching the real process environment or a variable (`XDG_CONFIG_HOME`)
/// a genuine install could plausibly have set for unrelated reasons.
#[test]
fn an_injected_override_replaces_the_real_os_config_directory() {
    let env = HashMap::from([(
        LEGACY_CONFIG_DIR_OVERRIDE.to_string(),
        "/scratch/config".to_string(),
    )]);

    assert_eq!(
        config_dir_location(&env),
        Some(std::path::PathBuf::from(
            "/scratch/config/medulla/credentials.json"
        ))
    );
}

/// An empty override is not a real override — the fallback still applies,
/// the same way `home_base` treats a blank `HOME` as absent.
#[test]
fn a_blank_override_is_ignored() {
    let env = HashMap::from([(LEGACY_CONFIG_DIR_OVERRIDE.to_string(), String::new())]);
    let expected = dirs::config_dir().map(|d| d.join("medulla").join("credentials.json"));

    assert_eq!(config_dir_location(&env), expected);
}

/// A missing home-location file is skipped rather than reported: the only
/// recovery from an unreadable legacy file is signing in again, and failing
/// the sweep would block exactly that.
#[test]
fn a_home_with_no_legacy_file_yields_nothing() {
    let root = tempfile::tempdir().unwrap();
    let env = HashMap::from([(
        LEGACY_CONFIG_DIR_OVERRIDE.to_string(),
        root.path().join("no-such-config").display().to_string(),
    )]);

    assert!(adopt_legacy_credentials(&env, root.path()).is_empty());
}

/// The home-location candidate is read and returned when it holds a real
/// token, with the file it came from carried alongside for later removal.
#[test]
fn a_legacy_credential_in_the_home_location_is_adopted() {
    let root = tempfile::tempdir().unwrap();
    let path = home_location(root.path());
    std::fs::write(
        &path,
        r#"{"baseUrl":"https://example.test","jwt":"a-token"}"#,
    )
    .unwrap();
    let env = HashMap::from([(
        LEGACY_CONFIG_DIR_OVERRIDE.to_string(),
        root.path().join("no-such-config").display().to_string(),
    )]);

    let found = adopt_legacy_credentials(&env, root.path());

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].path, path);
    assert_eq!(found[0].jwt, "a-token");
    assert_eq!(found[0].base_url, "https://example.test");
}

/// A blank JWT is not a credential worth adopting — a file with an empty
/// token field is treated the same as one that was never there.
#[test]
fn a_blank_jwt_is_not_adopted() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        home_location(root.path()),
        r#"{"baseUrl":"https://example.test","jwt":"  "}"#,
    )
    .unwrap();
    let env = HashMap::from([(
        LEGACY_CONFIG_DIR_OVERRIDE.to_string(),
        root.path().join("no-such-config").display().to_string(),
    )]);

    assert!(adopt_legacy_credentials(&env, root.path()).is_empty());
}

/// Malformed JSON is skipped, not reported — best-effort, since the sweep
/// rides along with commands that must not fail because of it.
#[test]
fn unparseable_json_is_skipped() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(home_location(root.path()), "not json").unwrap();
    let env = HashMap::from([(
        LEGACY_CONFIG_DIR_OVERRIDE.to_string(),
        root.path().join("no-such-config").display().to_string(),
    )]);

    assert!(adopt_legacy_credentials(&env, root.path()).is_empty());
}

/// Both candidate locations can hold a legacy credential at once, and both
/// come back, newest (home) location first.
#[test]
fn both_locations_are_reported_when_both_hold_a_credential() {
    let root = tempfile::tempdir().unwrap();
    let config_root = root.path().join("legacy-config");
    std::fs::write(
        home_location(root.path()),
        r#"{"baseUrl":"https://a.test","jwt":"home-token"}"#,
    )
    .unwrap();
    let env = HashMap::from([(
        LEGACY_CONFIG_DIR_OVERRIDE.to_string(),
        config_root.display().to_string(),
    )]);
    let config_path = config_dir_location(&env).unwrap();
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(
        &config_path,
        r#"{"baseUrl":"https://b.test","jwt":"config-token"}"#,
    )
    .unwrap();

    let found = adopt_legacy_credentials(&env, root.path());

    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0].jwt, "home-token");
    assert_eq!(found[1].jwt, "config-token");
}

/// Removing a file that exists succeeds and reports so.
#[test]
fn discarding_an_existing_file_removes_it() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("credentials.json");
    std::fs::write(&path, "{}").unwrap();

    assert!(discard_legacy_credential(&path));
    assert!(!path.exists());
}

/// A file that was never there was not removed by this call — the boolean
/// reports what this call did, not whether anything is left to sign the
/// operator back in with. A caller wanting the latter checks existence
/// itself, as `sign_out` (`src/tui/src/access_gate.rs`) does.
#[test]
fn discarding_a_missing_file_reports_it_removed_nothing() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("never-existed.json");

    assert!(!discard_legacy_credential(&path));
}
