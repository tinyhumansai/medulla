//! Unit tests for the wrapper root: argument parsing, provider→agent-kind
//! mapping, recipient resolution precedence, session-id minting, and the
//! missing-binary error path.

use std::collections::HashMap;

use crate::session_history::SessionAgentKind;
use crate::tinyplace::HarnessProvider;

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
        pty_spawner: None,
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("not found on PATH"), "got: {err}");
}

/// Two pinned tailers, one directory, two transcripts — each latches only its
/// own. This is the mechanism that makes **claude** immune to the concurrent
/// swap that afflicts codex: claude is launched with a minted `--session-id`, so
/// its tailer pins to that id and matches by identity, never by recency. A codex
/// tailer cannot pin (codex self-mints), which is the open gap documented at the
/// executor's claim set.
#[test]
fn pinned_tailers_latch_by_identity_never_swapping() {
    use super::tail::SessionTailer;

    let dir = tempfile::tempdir().unwrap();
    let here = dir.path().to_string_lossy().into_owned();
    let env: HashMap<String, String> = [("TINYPLACE_CODEX_SESSIONS_DIR".to_string(), here.clone())]
        .into_iter()
        .collect();

    // Tailers are built before the transcripts exist, exactly as the executor
    // builds them before the harness writes. Each is pinned to a different id,
    // as claude's minted ids are.
    let mut tail_a =
        SessionTailer::new(env.clone(), SessionAgentKind::Codex, &here, 0).expecting("sess-a");
    let mut tail_b = SessionTailer::new(env, SessionAgentKind::Codex, &here, 0).expecting("sess-b");

    // Now both transcripts appear in the shared directory.
    for (name, id) in [("rollout-a.jsonl", "sess-a"), ("rollout-b.jsonl", "sess-b")] {
        let line = serde_json::json!({
            "type": "session_meta",
            "payload": { "session_id": id, "cwd": here }
        });
        std::fs::write(dir.path().join(name), format!("{line}\n")).unwrap();
    }

    let a = tail_a
        .poll()
        .located
        .expect("tail_a locates its own transcript");
    let b = tail_b
        .poll()
        .located
        .expect("tail_b locates its own transcript");
    assert_eq!(a.harness_session_id, "sess-a");
    assert_eq!(b.harness_session_id, "sess-b");
    assert_ne!(a.path, b.path, "pinned tailers never share a transcript");
}
