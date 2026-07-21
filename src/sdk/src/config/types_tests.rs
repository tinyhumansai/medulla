//! Unit tests for the config data model: serde defaults/parsing, derived
//! labels, and core-socket path/request resolution on [`LoadedConfig`].

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
fn update_config_enabled_honors_config_and_env() {
    // Default: on.
    let cfg = UpdateConfig::default();
    assert!(cfg.enabled(&env(&[])));
    // Config kill-switch.
    let off = UpdateConfig { check: false };
    assert!(!off.enabled(&env(&[])));
    // Env kill-switch overrides an on config.
    assert!(!cfg.enabled(&env(&[("MEDULLA_NO_UPDATE_CHECK", "1")])));
    // "0" / empty are treated as unset.
    assert!(cfg.enabled(&env(&[("MEDULLA_NO_UPDATE_CHECK", "0")])));
    assert!(cfg.enabled(&env(&[("MEDULLA_NO_UPDATE_CHECK", "")])));
}

#[test]
fn defaults_are_applied() {
    // Serde defaults (no env resolution) produce the PROD urls and the
    // home-less state-dir placeholder (real value filled by load_config).
    let cfg: TuiConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(cfg.state_dir, "state");
    assert_eq!(cfg.backend.base_url, "https://api.tinyhumans.ai");
    assert_eq!(cfg.backend.token_env, "MEDULLA_TOKEN");
    assert_eq!(cfg.medulla.context_window(), 32_000);
}

#[test]
fn backend_and_tinyplace_parse() {
    let cfg: TuiConfig = serde_json::from_str(
        r#"{"backend":{"baseUrl":"http://x:1","token":"t"},"tinyplace":{"peers":[{"id":"p1","handle":"@a"}]}}"#,
    )
    .unwrap();
    assert_eq!(cfg.backend.base_url, "http://x:1");
    assert_eq!(cfg.backend.token.as_deref(), Some("t"));
    let tp = cfg.tinyplace.unwrap();
    assert_eq!(tp.peers.len(), 1);
    assert_eq!(tp.peers[0].protocol, "task");
    // Serde default (no env resolution) is the prod tiny.place URL.
    assert_eq!(tp.base_url, "https://api.tiny.place");
}

#[test]
fn harness_label() {
    let mut loaded = LoadedConfig::defaults("x".into());
    loaded.config.opencode = Some(OpencodeConfig {
        command: "/usr/bin/opencode".into(),
        ..Default::default()
    });
    assert_eq!(loaded.harness(), "OPENCODE");
    loaded.config.tinyplace = Some(TinyplaceConfig::default());
    assert_eq!(loaded.harness(), "TINYPLACE");
}

#[test]
fn pretty_json_annotates_token_env() {
    let loaded = LoadedConfig::defaults("x".into());
    let json = loaded.pretty_json();
    assert!(json.contains("MEDULLA_TOKEN ("));
}

#[test]
fn pretty_json_marks_token_set_when_env_present() {
    let var = "MEDULLA_CONFIG_TEST_TOKEN";
    std::env::set_var(var, "value");
    let mut loaded = LoadedConfig::defaults("x".into());
    loaded.config.backend.token_env = var.into();
    assert!(loaded.pretty_json().contains(&format!("{var} (set)")));
    std::env::remove_var(var);
    assert!(loaded.pretty_json().contains(&format!("{var} (missing)")));
}

#[test]
fn harness_defaults_to_worker_without_backends() {
    // No tinyplace and no opencode → the generic WORKER label.
    let loaded = LoadedConfig::defaults("x".into());
    assert_eq!(loaded.harness(), "WORKER");
}

#[test]
fn harness_opencode_bare_command_and_empty() {
    let mut loaded = LoadedConfig::defaults("x".into());
    loaded.config.opencode = Some(OpencodeConfig {
        command: "codex".into(),
        ..Default::default()
    });
    assert_eq!(loaded.harness(), "CODEX");
    // A trailing-slash / empty basename falls back to WORKER.
    loaded.config.opencode = Some(OpencodeConfig {
        command: "bin/".into(),
        ..Default::default()
    });
    assert_eq!(loaded.harness(), "WORKER");
}

#[test]
fn context_window_honors_override() {
    let cfg: TuiConfig =
        serde_json::from_str(r#"{"medulla":{"contextWindowTokens":128000}}"#).unwrap();
    assert_eq!(cfg.medulla.context_window(), 128_000);
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

    // A `--core-socket` flag opts in and wins over everything else.
    let cfg_with_core: TuiConfig =
        serde_json::from_str(r#"{"core":{"socketPath":"/tmp/from-config.sock"}}"#).unwrap();
    let loaded = LoadedConfig {
        config: cfg_with_core,
        path: "x".into(),
        sources: Vec::new(),
    };
    assert_eq!(
        loaded.core_socket_request(
            &env(&[("MEDULLA_CORE_SOCKET", "/tmp/from-env.sock")]),
            Some("/tmp/from-flag.sock"),
        ),
        Some(PathBuf::from("/tmp/from-flag.sock"))
    );
    // A blank flag is treated as unset, so the env var wins next.
    assert_eq!(
        loaded.core_socket_request(
            &env(&[("MEDULLA_CORE_SOCKET", "/tmp/from-env.sock")]),
            Some(" ")
        ),
        Some(PathBuf::from("/tmp/from-env.sock"))
    );
    // With neither flag nor env, the presence of [core] opts in and the path is
    // resolved through `core_socket_path` (explicit socketPath here).
    assert_eq!(
        loaded.core_socket_request(&env(&[]), None),
        Some(PathBuf::from("/tmp/from-config.sock"))
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

#[test]
fn core_section_round_trips_and_omits_when_absent() {
    // Present socketPath deserializes; absent [core] serializes to nothing.
    let cfg: TuiConfig =
        serde_json::from_str(r#"{"core":{"socketPath":"/run/serve.sock"}}"#).unwrap();
    assert_eq!(
        cfg.core.as_ref().unwrap().socket_path.as_deref(),
        Some("/run/serve.sock")
    );
    let bare = TuiConfig::default();
    let json = serde_json::to_value(&bare).unwrap();
    assert!(json.get("core").is_none(), "absent core must be omitted");
}

#[test]
fn unknown_fields_are_ignored() {
    // Permissive parsing: extra keys (including retired sections like
    // `inference`/`langfuse`) must not fail the load.
    let cfg: TuiConfig = serde_json::from_str(
        r#"{"totallyUnknown":true,"inference":{"temperature":0.9},"langfuse":{"enabled":true},"medulla":{"maxPasses":3}}"#,
    )
    .unwrap();
    assert_eq!(cfg.medulla.max_passes, Some(3));
}

#[test]
fn memory_section_parses_camel_case() {
    let cfg: TuiConfig = serde_json::from_str(
        r#"{"memory":{"enabled":true,"workspace":"/ws","identity":"a@b","projectRoots":["/x","/y"],"model":"m","maxCostUsd":3.0}}"#,
    )
    .unwrap();
    let mem = cfg.memory.unwrap();
    assert_eq!(mem.enabled, Some(true));
    assert_eq!(mem.workspace.as_deref(), Some("/ws"));
    assert_eq!(mem.identity.as_deref(), Some("a@b"));
    assert_eq!(mem.project_roots, vec!["/x".to_string(), "/y".to_string()]);
    assert_eq!(mem.model.as_deref(), Some("m"));
    assert_eq!(mem.max_cost_usd, Some(3.0));
    // Absent by default.
    let bare: TuiConfig = serde_json::from_str("{}").unwrap();
    assert!(bare.memory.is_none());
}

#[test]
fn peer_protocol_defaults_to_task() {
    let peer: Peer = serde_json::from_str(r#"{"id":"p1"}"#).unwrap();
    assert_eq!(peer.protocol, "task");
}
