//! Socket path resolution and bind safety.

use std::collections::HashMap;

#[cfg(unix)]
use super::super::path::ControlSocketError;
use super::super::path::{control_socket_path, CONTROL_SOCKET_ENV};

/// An environment map from pairs.
fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn an_explicit_environment_path_wins_over_everything() {
    let resolved = control_socket_path(
        &env(&[
            (CONTROL_SOCKET_ENV, "/run/explicit.sock"),
            ("MEDULLA_HOME", "/tmp/medulla-root"),
        ]),
        Some("/from/config.sock"),
    )
    .unwrap();

    assert_eq!(resolved, std::path::PathBuf::from("/run/explicit.sock"));
}

#[test]
fn config_wins_over_the_home_derived_default() {
    let resolved = control_socket_path(
        &env(&[("MEDULLA_HOME", "/tmp/medulla-root")]),
        Some("/from/config.sock"),
    )
    .unwrap();

    assert_eq!(resolved, std::path::PathBuf::from("/from/config.sock"));
}

#[test]
fn a_blank_override_does_not_mask_the_next_source() {
    // An exported-but-empty variable is the shape a shell script leaves behind,
    // and treating it as a real path would resolve the socket to "".
    let resolved = control_socket_path(
        &env(&[
            (CONTROL_SOCKET_ENV, "   "),
            ("MEDULLA_HOME", "/tmp/medulla-root"),
        ]),
        Some("   "),
    )
    .unwrap();

    assert!(resolved.starts_with("/tmp/medulla-root"));
    assert!(resolved.ends_with("control.sock"));
}

#[test]
fn the_default_is_scoped_to_the_account_not_the_machine() {
    // Two accounts under one root must not share a fleet: a dispatch made by one
    // operator's session landing in another's is not a bug you notice quickly.
    let first = control_socket_path(
        &env(&[("MEDULLA_HOME", "/tmp/root"), ("MEDULLA_USER", "alice")]),
        None,
    )
    .unwrap();
    let second = control_socket_path(
        &env(&[("MEDULLA_HOME", "/tmp/root"), ("MEDULLA_USER", "bob")]),
        None,
    )
    .unwrap();

    assert_ne!(first, second);
}

#[test]
fn an_overlong_home_falls_back_to_a_path_that_fits() {
    // `sun_path` holds 104 bytes on macOS. A long `$HOME` with an account id
    // underneath overflows it, and the kernel's answer is a bare EINVAL, so the
    // fallback is mandatory rather than defensive.
    let long_root = format!("/tmp/{}", "d".repeat(120));
    let resolved = control_socket_path(
        &env(&[
            ("MEDULLA_HOME", long_root.as_str()),
            ("MEDULLA_USER", "alice"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]),
        None,
    )
    .unwrap();

    assert!(
        resolved.as_os_str().len() <= 103,
        "{resolved:?} still too long"
    );
    assert!(resolved.starts_with("/run/user/1000"));
}

#[test]
fn the_fallback_stays_account_distinct() {
    // The whole point of falling back is to keep working, not to start sharing:
    // two accounts that both overflow must still land on different sockets.
    let long_root = format!("/tmp/{}", "d".repeat(120));
    let base = [
        ("MEDULLA_HOME", long_root.as_str()),
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
    ];

    let first =
        control_socket_path(&env(&[base[0], base[1], ("MEDULLA_USER", "alice")]), None).unwrap();
    let second =
        control_socket_path(&env(&[base[0], base[1], ("MEDULLA_USER", "bob")]), None).unwrap();

    assert_ne!(first, second);
}

#[test]
fn without_a_runtime_dir_the_fallback_uses_the_temp_dir() {
    let long_root = format!("/tmp/{}", "d".repeat(120));
    let resolved = control_socket_path(
        &env(&[
            ("MEDULLA_HOME", long_root.as_str()),
            ("MEDULLA_USER", "alice"),
            ("TMPDIR", "/tmp"),
        ]),
        None,
    )
    .unwrap();

    assert!(resolved.as_os_str().len() <= 103);
    // A string prefix, not `Path::starts_with`: that compares whole path
    // components, so a partial component like "medulla-" never matches.
    assert!(resolved.to_string_lossy().starts_with("/tmp/medulla-"));
}

#[cfg(unix)]
mod bind {
    use super::super::super::path::prepare_bind;
    use super::*;

    #[tokio::test]
    async fn a_free_path_is_bindable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.sock");

        assert!(prepare_bind(&path, false).await.is_ok());
    }

    #[tokio::test]
    async fn a_regular_file_is_refused_and_never_deleted() {
        // Somebody's data. A control plane that unlinks unfamiliar files to get
        // its address is worse than one that declines to start.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.sock");
        std::fs::write(&path, b"not a socket").unwrap();

        let result = prepare_bind(&path, false).await;

        assert!(matches!(result, Err(ControlSocketError::NotASocket(_))));
        assert_eq!(std::fs::read(&path).unwrap(), b"not a socket");
    }

    #[tokio::test]
    async fn a_dead_socket_is_reaped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.sock");
        // Bind and drop: the file survives, but nothing answers on it.
        drop(tokio::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists());

        assert!(prepare_bind(&path, false).await.is_ok());
        assert!(!path.exists(), "the stale socket should have been unlinked");
    }

    #[tokio::test]
    async fn a_live_socket_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.sock");
        let _listener = tokio::net::UnixListener::bind(&path).unwrap();

        let result = prepare_bind(&path, false).await;

        assert!(matches!(result, Err(ControlSocketError::AlreadyBound(_))));
        assert!(path.exists(), "a live instance's socket must survive");
    }

    #[tokio::test]
    async fn the_parent_directory_is_created_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("control.sock");

        prepare_bind(&path, false).await.unwrap();

        let mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "parent should be owner-only");
    }

    #[tokio::test]
    async fn an_explicit_paths_existing_parent_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        let path = shared.join("control.sock");

        prepare_bind(&path, true).await.unwrap();

        let mode = std::fs::metadata(&shared).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o777, "shared parent must not be chmodded");
    }
}
