//! Unit tests for the wrapper root: argument parsing, provider→agent-kind
//! mapping, recipient resolution precedence, session-id minting, and the
//! missing-binary error path.

use std::collections::HashMap;

use crate::session_history::SessionAgentKind;
use crate::tinyplace_support::HarnessProvider;

use super::args::parse_wrapper_args;
use super::bridge::{agent_kind, mint_session_id, resolve_recipient};
use super::run::run_wrapper_with;
use super::types::WrapperConfig;

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

#[test]
fn parse_wrapper_args_strips_no_bridge_and_passes_rest() {
    let (no_bridge, child) = parse_wrapper_args(&argv(&["--no-bridge", "resume", "--model", "x"]));
    assert!(no_bridge);
    assert_eq!(child, vec!["resume", "--model", "x"]);

    let (no_bridge, child) = parse_wrapper_args(&argv(&["exec", "--json"]));
    assert!(!no_bridge);
    assert_eq!(child, vec!["exec", "--json"]);
}

#[test]
fn double_dash_forces_passthrough_including_no_bridge() {
    let (no_bridge, child) = parse_wrapper_args(&argv(&["--", "--no-bridge", "--flag"]));
    assert!(!no_bridge, "after -- everything is the child's");
    assert_eq!(child, vec!["--no-bridge", "--flag"]);
}

#[test]
fn agent_kind_maps_providers() {
    assert_eq!(
        agent_kind(HarnessProvider::Claude),
        Some(SessionAgentKind::Claude)
    );
    assert_eq!(
        agent_kind(HarnessProvider::Codex),
        Some(SessionAgentKind::Codex)
    );
    assert_eq!(agent_kind(HarnessProvider::Opencode), None);
}

#[test]
fn profile_owner_is_the_last_recipient_fallback() {
    // No env owner → the profile owner is used.
    let env = HashMap::new();
    assert_eq!(
        resolve_recipient(HarnessProvider::Codex, &env, Some("@profile-owner")).as_deref(),
        Some("@profile-owner")
    );
    // An env owner beats the profile owner.
    let mut env = HashMap::new();
    env.insert(
        "TINYPLACE_OPENHUMAN_OWNER".to_string(),
        "@env-owner".to_string(),
    );
    assert_eq!(
        resolve_recipient(HarnessProvider::Codex, &env, Some("@profile-owner")).as_deref(),
        Some("@env-owner")
    );
    // An empty profile owner is treated as absent.
    let env = HashMap::new();
    assert_eq!(
        resolve_recipient(HarnessProvider::Codex, &env, Some("")),
        None
    );
    assert_eq!(resolve_recipient(HarnessProvider::Codex, &env, None), None);
}

#[test]
fn mint_session_id_is_id_safe_and_prefixed() {
    let id = mint_session_id(HarnessProvider::Codex);
    assert!(id.starts_with("tp-codex-"));
    assert!(!id.contains(':'));
    assert!(!id.contains('.'));
}

#[tokio::test]
async fn missing_binary_is_a_clear_error() {
    let mut env = HashMap::new();
    env.insert("PATH".to_string(), "/nonexistent".to_string());
    env.insert(
        "TINYPLACE_CODEX_BIN".to_string(),
        "/no/such/codex-binary".to_string(),
    );
    let err = run_wrapper_with(WrapperConfig {
        provider: HarnessProvider::Codex,
        child_args: Vec::new(),
        env,
        cwd: ".".to_string(),
        no_bridge: true,
        session_id: Some("wsid-test".to_string()),
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("not found on PATH"), "got: {err}");
}
