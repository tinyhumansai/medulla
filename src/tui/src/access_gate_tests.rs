//! Unit tests for the launch-time access gate's parts that need no terminal:
//! the startup note a grant is announced with, and the sign-out the gate screen
//! offers.

use std::collections::HashMap;

use medulla::access::{Grant, Plan};

use super::{sign_out, startup_note};

const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

#[test]
fn a_paid_account_is_not_told_anything() {
    assert_eq!(startup_note(Grant::Paid(Plan::Basic)), None);
    assert_eq!(startup_note(Grant::Paid(Plan::Pro)), None);
}

#[test]
fn an_admin_grant_is_not_announced() {
    assert_eq!(startup_note(Grant::AdminGranted), None);
}

#[test]
fn a_trial_counts_the_days_down() {
    let note = startup_note(Grant::Trial {
        remaining_ms: 5 * DAY_MS,
    });
    assert_eq!(
        note.as_deref(),
        Some(
            "Free trial: 5 days left. Medulla needs a Basic subscription after that — \
             https://tinyhumans.ai/pricing"
        )
    );
}

/// Rounding up, so a partial day is never reported as none left.
#[test]
fn a_partial_day_rounds_up_rather_than_reading_as_expired() {
    let note = startup_note(Grant::Trial {
        remaining_ms: 11 * 60 * 60 * 1_000,
    });
    assert!(note.unwrap().starts_with("Free trial: 1 day left."));
}

#[test]
fn the_last_day_is_phrased_in_the_singular() {
    let exactly_one = startup_note(Grant::Trial {
        remaining_ms: DAY_MS,
    });
    assert!(exactly_one.unwrap().starts_with("Free trial: 1 day left"));

    let two = startup_note(Grant::Trial {
        remaining_ms: 2 * DAY_MS,
    });
    assert!(two.unwrap().starts_with("Free trial: 2 days left"));
}

#[test]
fn a_full_window_reads_as_thirty_days() {
    let note = startup_note(Grant::Trial {
        remaining_ms: medulla::access::TRIAL.as_millis() as i64,
    });
    assert!(note.unwrap().starts_with("Free trial: 30 days left"));
}

/// The countdown is the only place a trialling operator is told what happens
/// when it runs out, so it has to say.
#[test]
fn the_countdown_names_the_subscription_it_ends_in() {
    let note = startup_note(Grant::Trial {
        remaining_ms: 3 * DAY_MS,
    })
    .unwrap();
    assert!(note.contains("Basic subscription"), "{note}");
    assert!(note.contains("https://tinyhumans.ai/pricing"), "{note}");
}

// --- signing out from the gate -------------------------------------------

/// An env pointing every Medulla path — including the OS config directory
/// `adopt_legacy_credentials` sweeps for a legacy credential — at scratch
/// roots, so a test never reads or removes anything the developer or CI
/// runner actually has.
///
/// `medulla::auth::LEGACY_CONFIG_DIR_OVERRIDE` is injected through this same
/// map rather than the real process environment. It is a dedicated key, not
/// `XDG_CONFIG_HOME`: a real install can have that set for reasons that have
/// nothing to do with Medulla, and repurposing it as the override would make
/// production sign-out miss the real legacy file such an install has at
/// `dirs::config_dir()`'s native location. Because nothing but a caller that
/// means to redirect this specific lookup would ever set the dedicated key,
/// setting it here isolates every sign-out test on every platform without
/// touching production resolution at all. See `src/sdk/src/auth/migrate.rs`.
fn scratch_env(root: &std::path::Path) -> HashMap<String, String> {
    HashMap::from([
        ("MEDULLA_HOME".to_string(), root.display().to_string()),
        (
            medulla::auth::LEGACY_CONFIG_DIR_OVERRIDE.to_string(),
            root.join("legacy-config").display().to_string(),
        ),
    ])
}

/// Write a session file where `medulla::auth::clear` will look for it.
fn seed_session(env: &HashMap<String, String>) -> std::path::PathBuf {
    let home = medulla::home::medulla_home(env);
    std::fs::create_dir_all(&home).unwrap();
    let path = home.join("session.json");
    std::fs::write(&path, r#"{"token":"t","userId":"u1"}"#).unwrap();
    path
}

/// Where `scratch_env`'s override puts the legacy credential file, matching
/// `config_dir_location`'s `<base>/medulla/credentials.json` layout.
fn legacy_credential_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join("legacy-config")
        .join("medulla")
        .join("credentials.json")
}

/// Write a legacy credential at the scratch OS-config-directory location
/// `scratch_env` points the override at.
fn seed_legacy_credential(root: &std::path::Path) -> std::path::PathBuf {
    let path = legacy_credential_path(root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"baseUrl":"https://example.test","jwt":"legacy-token","userId":"u1"}"#,
    )
    .unwrap();
    path
}

#[test]
fn signing_out_removes_the_stored_session() {
    let root = tempfile::tempdir().unwrap();
    let env = scratch_env(root.path());
    let path = seed_session(&env);

    let msg = sign_out(&env, &medulla::config::BackendConfig::default());

    assert!(!path.exists(), "the bearer is still on disk");
    assert_eq!(msg, "Signed out. Run `medulla` to sign in again.");
}

/// Idempotent, the same way `medulla logout` is: nothing stored is not a
/// failure to report.
#[test]
fn signing_out_with_nothing_stored_still_reports_a_clean_sign_out() {
    let root = tempfile::tempdir().unwrap();

    let msg = sign_out(
        &scratch_env(root.path()),
        &medulla::config::BackendConfig::default(),
    );

    assert_eq!(msg, "Signed out. Run `medulla` to sign in again.");
}

/// A token from the environment outranks the store, so clearing the store
/// leaves the next launch signed in as the same account — and landing back on
/// this screen would look like the sign-out silently failed.
#[test]
fn an_environment_token_is_reported_because_it_survives_the_sign_out() {
    let root = tempfile::tempdir().unwrap();
    let mut env = scratch_env(root.path());
    let backend = medulla::config::BackendConfig::default();
    env.insert(backend.token_env.clone(), "still-here".to_string());
    let path = seed_session(&env);

    let msg = sign_out(&env, &backend);

    // The removal still happens: what remains is a source this cannot reach.
    assert!(!path.exists(), "the stored session was left behind");
    assert!(msg.contains(&backend.token_env), "{msg}");
    assert!(msg.contains("still authenticated"), "{msg}");
}

#[test]
fn an_inline_config_token_is_named_as_the_config_rather_than_the_environment() {
    let root = tempfile::tempdir().unwrap();
    let backend = medulla::config::BackendConfig {
        token: Some("inline".to_string()),
        ..Default::default()
    };

    let msg = sign_out(&scratch_env(root.path()), &backend);

    assert!(msg.contains("`backend.token` in the config"), "{msg}");
}

/// A legacy `credentials.json` still in the OS config directory (the older of
/// the two locations `adopt_legacy_credentials` sweeps) is removed by signing
/// out, exactly as `medulla logout` removes it — so a re-launch cannot adopt
/// it and sign the operator straight back in.
#[test]
fn signing_out_removes_a_legacy_credential_from_the_config_directory() {
    let root = tempfile::tempdir().unwrap();
    let legacy_path = seed_legacy_credential(root.path());

    let msg = sign_out(
        &scratch_env(root.path()),
        &medulla::config::BackendConfig::default(),
    );

    assert!(
        !legacy_path.exists(),
        "the legacy credential is still on disk"
    );
    assert_eq!(msg, "Signed out. Run `medulla` to sign in again.");
}

/// Whether this process can bypass a directory's write permission (root, or
/// an equivalent capability inside some containers). The permission-based
/// removal-failure simulation below only proves anything when it cannot.
#[cfg(unix)]
fn can_bypass_directory_permissions() -> bool {
    // SAFETY: `geteuid` takes no arguments and has no safety preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// A legacy credential that cannot be removed — a read-only mount, a
/// permissions problem — must not be reported as a clean sign-out: that file
/// is exactly the bearer a re-launch would adopt, signing the operator back in
/// to the account they just left. Simulated here by making its directory
/// non-writable, which is Unix-only, and only meaningful when this process
/// cannot bypass that permission the way root (common inside containers) can.
#[cfg(unix)]
#[test]
fn a_legacy_credential_that_cannot_be_removed_is_named_in_the_parting_message() {
    use std::os::unix::fs::PermissionsExt;

    if can_bypass_directory_permissions() {
        eprintln!(
            "skipping a_legacy_credential_that_cannot_be_removed_is_named_in_the_parting_message: \
             running as a user that can unlink through a read-only directory (e.g. root), so the \
             permission-based failure this test simulates cannot occur"
        );
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let legacy_path = seed_legacy_credential(root.path());
    let legacy_dir = legacy_path.parent().unwrap();
    // Deny write on the directory: the file itself stays readable (so it is
    // still adopted as a candidate), but `remove_file` cannot unlink it.
    std::fs::set_permissions(legacy_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let msg = sign_out(
        &scratch_env(root.path()),
        &medulla::config::BackendConfig::default(),
    );

    // Restore write access so the temp directory can be cleaned up.
    std::fs::set_permissions(legacy_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(legacy_path.exists(), "the legacy credential should survive");
    assert!(
        msg.contains("could not be removed"),
        "the parting message must say the removal failed: {msg}"
    );
    assert!(msg.contains("credentials.json"), "{msg}");
}

/// An external token and an undeletable legacy credential are two independent
/// surviving authentication sources — either one alone would sign the operator
/// back in. Fixing only the one named in the message would still leave them
/// signed back in by the other, so both have to be named together.
#[cfg(unix)]
#[test]
fn both_a_surviving_token_and_an_undeletable_legacy_credential_are_reported() {
    use std::os::unix::fs::PermissionsExt;

    if can_bypass_directory_permissions() {
        eprintln!(
            "skipping both_a_surviving_token_and_an_undeletable_legacy_credential_are_reported: \
             running as a user that can unlink through a read-only directory (e.g. root), so the \
             permission-based failure this test simulates cannot occur"
        );
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let backend = medulla::config::BackendConfig::default();
    let mut env = scratch_env(root.path());
    env.insert(backend.token_env.clone(), "still-here".to_string());

    let legacy_path = seed_legacy_credential(root.path());
    let legacy_dir = legacy_path.parent().unwrap();
    std::fs::set_permissions(legacy_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let msg = sign_out(&env, &backend);

    std::fs::set_permissions(legacy_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(msg.contains(&backend.token_env), "{msg}");
    assert!(msg.contains("still authenticated"), "{msg}");
    assert!(msg.contains("could not be removed"), "{msg}");
    assert!(msg.contains("credentials.json"), "{msg}");
}
