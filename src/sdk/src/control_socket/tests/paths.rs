//! Socket path resolution and bind safety.

use std::collections::HashMap;

#[cfg(unix)]
use super::super::path::ControlSocketError;
use super::super::path::{absolute_path_from, control_socket_path, CONTROL_SOCKET_ENV};

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
fn a_relative_override_is_anchored_before_child_processes_receive_it() {
    let cwd = std::path::Path::new("/short/worktree");
    let resolved = absolute_path_from(std::path::PathBuf::from("run/control.sock"), cwd);

    assert!(resolved.is_absolute());
    assert_eq!(resolved, cwd.join("run/control.sock"));
}

#[test]
fn an_overlong_explicit_path_is_rejected_at_resolution() {
    let path = format!("/tmp/{}/control.sock", "x".repeat(120));

    assert!(matches!(
        control_socket_path(&env(&[(CONTROL_SOCKET_ENV, &path)]), None),
        Err(super::super::path::ControlSocketError::NoViablePath)
    ));
}

#[test]
fn a_relative_home_also_produces_an_anchored_socket_path() {
    let resolved = control_socket_path(&env(&[("MEDULLA_HOME", "relative-home")]), None).unwrap();

    assert!(resolved.is_absolute());
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
    // Compare path components so the assertion is independent of the host's
    // path separator (the SDK library tests also run on Windows).
    assert!(resolved.starts_with(std::path::Path::new("/tmp")));
    assert!(resolved
        .parent()
        .and_then(std::path::Path::file_name)
        .is_some_and(|name| name.to_string_lossy().starts_with("medulla-")));
}

#[cfg(unix)]
mod bind {
    use super::super::super::path::{prepare_bind, trusted_lock_owner, trusted_sticky_owner};
    use super::*;

    #[tokio::test]
    async fn a_free_path_is_bindable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.sock");

        assert!(prepare_bind(&path).await.is_ok());
    }

    #[tokio::test]
    async fn a_regular_file_is_refused_and_never_deleted() {
        // Somebody's data. A control plane that unlinks unfamiliar files to get
        // its address is worse than one that declines to start.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.sock");
        std::fs::write(&path, b"not a socket").unwrap();

        let result = prepare_bind(&path).await;

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

        assert!(prepare_bind(&path).await.is_ok());
        assert!(!path.exists(), "the stale socket should have been unlinked");
    }

    #[tokio::test]
    async fn bind_preparation_is_serialized_until_the_caller_binds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.sock");
        let first = prepare_bind(&path).await.unwrap();
        let competing_path = path.clone();
        let mut competing = tokio::spawn(async move { prepare_bind(&competing_path).await });

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut competing)
                .await
                .is_err(),
            "a second starter must wait while reclamation can still race bind"
        );
        drop(first);

        let second = tokio::time::timeout(std::time::Duration::from_secs(1), competing)
            .await
            .expect("the lock should release with its guard")
            .expect("the competing starter should not panic")
            .expect("the free path remains bindable");
        drop(second);
    }

    #[tokio::test]
    async fn a_stuck_bind_lock_is_bounded_instead_of_hanging_startup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.sock");
        let _held = prepare_bind(&path).await.unwrap();
        let started = std::time::Instant::now();

        let result = prepare_bind(&path).await;

        assert!(matches!(result, Err(ControlSocketError::AlreadyBound(_))));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "bind-lock contention must not hang startup"
        );
    }

    #[tokio::test]
    async fn a_live_socket_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.sock");
        let _listener = tokio::net::UnixListener::bind(&path).unwrap();

        let result = prepare_bind(&path).await;

        assert!(matches!(result, Err(ControlSocketError::AlreadyBound(_))));
        assert!(path.exists(), "a live instance's socket must survive");
    }

    #[tokio::test]
    async fn the_parent_directory_is_created_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("control.sock");

        drop(prepare_bind(&path).await.unwrap());

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
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o1777)).unwrap();
        let path = shared.join("control.sock");

        drop(prepare_bind(&path).await.unwrap());

        let mode = std::fs::metadata(&shared).unwrap().permissions().mode();
        assert_eq!(mode & 0o1777, 0o1777, "shared parent must not be chmodded");
    }

    #[tokio::test]
    async fn a_derived_paths_existing_symlinked_parent_is_never_chmodded() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let actual = dir.path().join("account-root");
        let linked = dir.path().join("linked-account-root");
        std::fs::create_dir(&actual).unwrap();
        std::fs::set_permissions(&actual, std::fs::Permissions::from_mode(0o750)).unwrap();
        symlink(&actual, &linked).unwrap();

        drop(prepare_bind(&linked.join("control.sock")).await.unwrap());

        assert_eq!(
            std::fs::metadata(&actual).unwrap().permissions().mode() & 0o777,
            0o750,
            "a derived path must preserve an existing symlink target"
        );
    }

    #[test]
    fn sticky_directories_are_trusted_only_for_this_user_or_root() {
        assert!(trusted_sticky_owner(1000, 1000));
        assert!(trusted_sticky_owner(0, 1000));
        assert!(!trusted_sticky_owner(2000, 1000));
        assert!(!trusted_sticky_owner(2000, 0));
    }

    #[test]
    fn an_existing_bind_lock_is_trusted_only_for_this_user() {
        assert!(trusted_lock_owner(1000, 1000));
        assert!(!trusted_lock_owner(2000, 1000));
        assert!(!trusted_lock_owner(0, 1000));
    }

    #[tokio::test]
    async fn an_explicit_path_refuses_a_replaceable_parent_without_chmodding_it() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        let path = shared.join("control.sock");

        let result = prepare_bind(&path).await;

        assert!(matches!(result, Err(ControlSocketError::InsecureParent(_))));
        let mode = std::fs::metadata(&shared).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o777, "refusal must not mutate the parent");
    }

    #[tokio::test]
    async fn a_symlink_cannot_hide_a_replaceable_parent() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let actual_parent = dir.path().join("replaceable");
        let private_parent = dir.path().join("private");
        std::fs::create_dir(&actual_parent).unwrap();
        std::fs::create_dir(&private_parent).unwrap();
        std::fs::set_permissions(&actual_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let linked_parent = private_parent.join("socket-parent");
        symlink(&actual_parent, &linked_parent).unwrap();

        let result = prepare_bind(&linked_parent.join("control.sock")).await;
        let resolved_actual_parent = std::fs::canonicalize(&actual_parent).unwrap();

        assert!(matches!(
            result,
            Err(ControlSocketError::InsecureParent(path)) if path == resolved_actual_parent
        ));
    }

    #[tokio::test]
    async fn an_explicit_path_refuses_a_private_parent_beneath_a_replaceable_ancestor() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        let private = shared.join("private");
        std::fs::create_dir_all(&private).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();

        let result = prepare_bind(&private.join("control.sock")).await;
        let resolved_shared = std::fs::canonicalize(&shared).unwrap();

        assert!(matches!(
            result,
            Err(ControlSocketError::InsecureParent(path)) if path == resolved_shared
        ));
        assert_eq!(
            std::fs::metadata(&private).unwrap().permissions().mode() & 0o777,
            0o700,
            "validation must preserve the explicit private parent"
        );
    }
}
