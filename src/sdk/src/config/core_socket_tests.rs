//! Unit tests for core-socket resolution and validation: the
//! path/request precedence on [`LoadedConfig`], the source naming, and the
//! fail-fast [`validate_core_socket`] boundary check.

use super::*;
use std::collections::HashMap;
use std::path::PathBuf;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn core_socket_path_prefers_explicit_over_xdg_and_state_dir() {
    // An explicit [core] socketPath wins over both fallbacks.
    let cfg: TuiConfig =
        serde_json::from_str(r#"{"core":{"socketPath":"/tmp/explicit.sock"}}"#).unwrap();
    let loaded = LoadedConfig {
        config: cfg,
        path: "x".into(),
        sources: Vec::new(),
    };
    let resolved = loaded.core_socket_path(&env(&[("XDG_RUNTIME_DIR", "/run/user/1000")]));
    assert_eq!(resolved, PathBuf::from("/tmp/explicit.sock"));
}

#[test]
fn core_socket_path_falls_back_to_xdg_then_state_dir() {
    // No explicit path: XDG_RUNTIME_DIR wins when set, else <stateDir>.
    let cfg = TuiConfig {
        state_dir: "/var/state".into(),
        ..Default::default()
    };
    let loaded = LoadedConfig {
        config: cfg,
        path: "x".into(),
        sources: Vec::new(),
    };
    // XDG present → $XDG_RUNTIME_DIR/medulla/serve.sock.
    assert_eq!(
        loaded.core_socket_path(&env(&[("XDG_RUNTIME_DIR", "/run/user/1000")])),
        PathBuf::from("/run/user/1000/medulla/serve.sock")
    );
    // XDG absent (and blank treated as unset) → <stateDir>/serve.sock.
    assert_eq!(
        loaded.core_socket_path(&env(&[("XDG_RUNTIME_DIR", "  ")])),
        PathBuf::from("/var/state/serve.sock")
    );
    assert_eq!(
        loaded.core_socket_path(&env(&[])),
        PathBuf::from("/var/state/serve.sock")
    );
}

#[test]
fn core_socket_path_treats_blank_explicit_as_unset() {
    // A whitespace-only socketPath must not shadow the fallbacks.
    let mut cfg: TuiConfig = serde_json::from_str(r#"{"core":{"socketPath":"   "}}"#).unwrap();
    cfg.state_dir = "/var/state".into();
    let loaded = LoadedConfig {
        config: cfg,
        path: "x".into(),
        sources: Vec::new(),
    };
    assert_eq!(
        loaded.core_socket_path(&env(&[])),
        PathBuf::from("/var/state/serve.sock")
    );
}

#[test]
fn core_socket_request_opts_in_only_when_asked() {
    // No [core] section and no flag/env: the core runtime is not requested, so
    // the caller keeps the backend/mock chain.
    let bare = LoadedConfig {
        config: TuiConfig {
            state_dir: "/var/state".into(),
            ..Default::default()
        },
        path: "x".into(),
        sources: Vec::new(),
    };
    assert_eq!(bare.core_socket_request(&env(&[]), None), None);
    assert_eq!(bare.core_socket_request_sourced(&env(&[]), None), None);

    // A `--core-socket` flag opts in and wins over everything else.
    let cfg_with_core: TuiConfig =
        serde_json::from_str(r#"{"core":{"socketPath":"/tmp/from-config.sock"}}"#).unwrap();
    let loaded = LoadedConfig {
        config: cfg_with_core,
        path: "x".into(),
        sources: Vec::new(),
    };
    assert_eq!(
        loaded.core_socket_request_sourced(
            &env(&[("MEDULLA_CORE_SOCKET", "/tmp/from-env.sock")]),
            Some("/tmp/from-flag.sock"),
        ),
        Some((
            PathBuf::from("/tmp/from-flag.sock"),
            CoreSocketSource::CliFlag
        ))
    );
    // A blank flag is treated as unset, so the env var wins next.
    assert_eq!(
        loaded.core_socket_request_sourced(
            &env(&[("MEDULLA_CORE_SOCKET", "/tmp/from-env.sock")]),
            Some(" ")
        ),
        Some((
            PathBuf::from("/tmp/from-env.sock"),
            CoreSocketSource::EnvVar
        ))
    );
    // With neither flag nor env, the presence of [core] opts in and the path is
    // resolved through `core_socket_path` (explicit socketPath here).
    assert_eq!(
        loaded.core_socket_request_sourced(&env(&[]), None),
        Some((
            PathBuf::from("/tmp/from-config.sock"),
            CoreSocketSource::ConfigSection
        ))
    );

    // A bare (empty) [core] section still opts in, falling back to the state dir.
    let empty_core: TuiConfig = serde_json::from_str(r#"{"core":{}}"#).unwrap();
    let loaded_empty = LoadedConfig {
        config: TuiConfig {
            state_dir: "/var/state".into(),
            ..empty_core
        },
        path: "x".into(),
        sources: Vec::new(),
    };
    assert_eq!(
        loaded_empty.core_socket_request(&env(&[]), None),
        Some(PathBuf::from("/var/state/serve.sock"))
    );
    // Even with no [core] section, the env var alone opts in.
    assert_eq!(
        bare.core_socket_request(&env(&[("MEDULLA_CORE_SOCKET", "/tmp/env-only.sock")]), None),
        Some(PathBuf::from("/tmp/env-only.sock"))
    );
}

#[cfg(unix)]
#[test]
fn validate_core_socket_accepts_a_bound_socket_and_a_missing_path() {
    // A real listening unix socket passes.
    let dir = tempfile::TempDir::new().unwrap();
    let sock = dir.path().join("serve.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    validate_core_socket(&sock, CoreSocketSource::CliFlag).expect("a bound socket is valid");

    // A missing path passes too: attach-before-serve waits for it to appear.
    let absent = dir.path().join("not-yet.sock");
    validate_core_socket(&absent, CoreSocketSource::EnvVar).expect("a missing path is allowed");
}

#[cfg(unix)]
#[test]
fn validate_core_socket_rejects_an_existing_non_socket() {
    // A regular file (or any non-socket) can never be attached: fail fast with
    // an error naming both the path and the knob that produced it (Codex review
    // finding — the driver would otherwise spin in reconnect forever).
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("not-a-socket");
    std::fs::write(&file, b"plain file").unwrap();

    let err = validate_core_socket(&file, CoreSocketSource::CliFlag)
        .expect_err("a regular file must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("not-a-socket"), "{msg}");
    assert!(msg.contains("--core-socket"), "{msg}");
    assert!(msg.contains("not a unix socket"), "{msg}");

    // A directory is rejected the same way, and each source names itself.
    let err = validate_core_socket(dir.path(), CoreSocketSource::EnvVar).unwrap_err();
    assert!(err.to_string().contains("MEDULLA_CORE_SOCKET"), "{err}");
    let err = validate_core_socket(dir.path(), CoreSocketSource::ConfigSection).unwrap_err();
    assert!(err.to_string().contains("[core] config section"), "{err}");
    let err = validate_core_socket(dir.path(), CoreSocketSource::DefaultPath).unwrap_err();
    assert!(
        err.to_string().contains("default runtime directory"),
        "{err}"
    );
}
