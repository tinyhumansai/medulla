//! Unit tests for the config data model: serde defaults/parsing and derived
//! labels on [`LoadedConfig`]. Core-socket resolution/validation tests live in
//! [`super::core_socket_tests`].

use super::*;
use std::collections::HashMap;

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
    assert!(cfg.workflow.workspaces.is_empty());
}

#[test]
fn workflow_workspaces_parse_for_daemon_workers() {
    let cfg: TuiConfig =
        serde_json::from_str(r#"{"workflow":{"workspaces":["/one","/two"]}}"#).unwrap();
    assert_eq!(cfg.workflow.workspaces, vec!["/one", "/two"]);
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

#[test]
fn router_absent_by_default_and_omitted_from_output() {
    // No [router] section → feature entirely off, and it never appears in
    // serialized output (zero behaviour change for configs that don't use it).
    let cfg: TuiConfig = serde_json::from_str("{}").unwrap();
    assert!(cfg.router.is_none());
    let json = serde_json::to_value(&cfg).unwrap();
    assert!(
        json.get("router").is_none(),
        "absent router must be omitted"
    );
}

#[test]
fn router_section_round_trips_camel_case() {
    // The exact published contract shape (matches medulla-v1's routerConfig
    // fixture and the backend's stored shape).
    let cfg: TuiConfig = serde_json::from_str(
        r#"{"router":{
            "baseUrl":"https://gateway.internal/v1",
            "apiKeyEnv":"MEDULLA_ROUTER_KEY",
            "models":{"reasoning":"gpt-tier-a","compress":"gpt-tier-c"},
            "providers":{"claude":{"baseUrl":"https://gateway.internal/anthropic"}}
        }}"#,
    )
    .unwrap();
    let router = cfg.router.clone().unwrap();
    assert_eq!(
        router.base_url.as_deref(),
        Some("https://gateway.internal/v1")
    );
    assert_eq!(router.api_key_env.as_deref(), Some("MEDULLA_ROUTER_KEY"));
    assert_eq!(router.model_for_tier("reasoning"), Some("gpt-tier-a"));
    assert_eq!(router.model_for_tier("compress"), Some("gpt-tier-c"));
    assert_eq!(router.model_for_tier("orchestrator"), None);

    // Re-serialize and confirm camelCase keys survive (never snake_case).
    let out = serde_json::to_string(&cfg).unwrap();
    assert!(out.contains("\"baseUrl\""));
    assert!(out.contains("\"apiKeyEnv\""));
    assert!(!out.contains("base_url"));
    assert!(!out.contains("api_key_env"));

    // Full round-trip preserves equality.
    let reparsed: TuiConfig = serde_json::from_str(&out).unwrap();
    assert_eq!(reparsed.router, cfg.router);
}

#[test]
fn router_base_url_precedence_provider_over_top_level() {
    // providers.<p>.baseUrl beats the top-level baseUrl; a provider with no
    // override inherits the top-level; an unconfigured router yields nothing.
    let router: RouterConfig = serde_json::from_str(
        r#"{
            "baseUrl":"https://top.example/v1",
            "providers":{"claude":{"baseUrl":"https://claude.example/anthropic"}}
        }"#,
    )
    .unwrap();
    // claude has an explicit override → provider URL wins.
    assert_eq!(
        router.base_url_for("claude"),
        Some("https://claude.example/anthropic")
    );
    // codex has no override → falls back to the top-level baseUrl.
    assert_eq!(router.base_url_for("codex"), Some("https://top.example/v1"));

    // With neither top-level nor provider baseUrl set, resolution is empty.
    let empty = RouterConfig::default();
    assert_eq!(empty.base_url_for("codex"), None);
}

#[test]
fn router_tolerates_unknown_fields() {
    // Permissive parsing: unknown keys inside [router] (and a future
    // provider-scoped field) must not fail the load.
    let cfg: TuiConfig = serde_json::from_str(
        r#"{"router":{"baseUrl":"https://x/v1","futureKnob":true,
            "providers":{"codex":{"baseUrl":"https://c/v1","weight":3}}}}"#,
    )
    .unwrap();
    let router = cfg.router.unwrap();
    assert_eq!(router.base_url_for("codex"), Some("https://c/v1"));
}
