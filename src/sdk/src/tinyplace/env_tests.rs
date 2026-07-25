//! Tests for the env module.

use super::*;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn dm_recipient_per_provider_beats_generic_beats_owner_fallbacks() {
    // Owner fallback chain, from lowest to highest precedence.
    let e = env(&[("OPENHUMAN_OWNER_AGENT", "legacy")]);
    assert_eq!(
        dm_recipient(HarnessProvider::Codex, &e).as_deref(),
        Some("legacy")
    );

    let e = env(&[
        ("OPENHUMAN_OWNER_AGENT", "legacy"),
        ("TINYPLACE_OPENHUMAN_OWNER", "owner"),
    ]);
    assert_eq!(
        dm_recipient(HarnessProvider::Codex, &e).as_deref(),
        Some("owner")
    );

    let e = env(&[
        ("TINYPLACE_OPENHUMAN_OWNER", "owner"),
        ("TINYPLACE_HARNESS_DM_TO", "harness"),
    ]);
    assert_eq!(
        dm_recipient(HarnessProvider::Codex, &e).as_deref(),
        Some("harness")
    );

    let e = env(&[
        ("TINYPLACE_HARNESS_DM_TO", "harness"),
        ("TINYPLACE_CODEX_DM_TO", "codex"),
    ]);
    assert_eq!(
        dm_recipient(HarnessProvider::Codex, &e).as_deref(),
        Some("codex")
    );
    // A per-provider key for a different provider does not leak.
    assert_eq!(
        dm_recipient(HarnessProvider::Claude, &e).as_deref(),
        Some("harness")
    );

    assert_eq!(dm_recipient(HarnessProvider::Codex, &env(&[])), None);
}

#[test]
fn empty_values_are_skipped() {
    let e = env(&[
        ("TINYPLACE_CODEX_DM_TO", ""),
        ("TINYPLACE_HARNESS_DM_TO", "harness"),
    ]);
    assert_eq!(
        dm_recipient(HarnessProvider::Codex, &e).as_deref(),
        Some("harness")
    );
}

#[test]
fn receive_from_falls_back_to_recipient() {
    // No receive-from keys → falls back to the passed recipient.
    assert_eq!(
        receive_from(HarnessProvider::Codex, &env(&[]), Some("owner")).as_deref(),
        Some("owner")
    );
    // Generic override wins over the recipient.
    let e = env(&[("TINYPLACE_HARNESS_RECEIVE_FROM", "generic")]);
    assert_eq!(
        receive_from(HarnessProvider::Codex, &e, Some("owner")).as_deref(),
        Some("generic")
    );
    // Per-provider beats generic.
    let e = env(&[
        ("TINYPLACE_HARNESS_RECEIVE_FROM", "generic"),
        ("TINYPLACE_CODEX_RECEIVE_FROM", "codex"),
    ]);
    assert_eq!(
        receive_from(HarnessProvider::Codex, &e, Some("owner")).as_deref(),
        Some("codex")
    );
    // No recipient and no keys → None.
    assert_eq!(receive_from(HarnessProvider::Codex, &env(&[]), None), None);
}

#[test]
fn receive_enabled_default_on_and_explicit_off() {
    assert!(receive_enabled(HarnessProvider::Claude, &env(&[])));
    // Generic off.
    let e = env(&[("TINYPLACE_HARNESS_RECEIVE", "0")]);
    assert!(!receive_enabled(HarnessProvider::Claude, &e));
    // Per-provider off beats a generic that is on.
    let e = env(&[
        ("TINYPLACE_HARNESS_RECEIVE", "1"),
        ("TINYPLACE_CLAUDE_RECEIVE", "0"),
    ]);
    assert!(!receive_enabled(HarnessProvider::Claude, &e));
    // Per-provider on beats a generic that is off.
    let e = env(&[
        ("TINYPLACE_HARNESS_RECEIVE", "0"),
        ("TINYPLACE_CLAUDE_RECEIVE", "1"),
    ]);
    assert!(receive_enabled(HarnessProvider::Claude, &e));
}

#[test]
fn provider_bin_override_and_default() {
    assert_eq!(provider_bin(HarnessProvider::Codex, &env(&[])), "codex");
    let e = env(&[("TINYPLACE_CODEX_BIN", "/opt/codex")]);
    assert_eq!(provider_bin(HarnessProvider::Codex, &e), "/opt/codex");
    // Claude honors TINYVERSE_* before TINYPLACE_*, and trims.
    let e = env(&[
        ("TINYVERSE_CLAUDE_BIN", "  /opt/claude  "),
        ("TINYPLACE_CLAUDE_BIN", "/other/claude"),
    ]);
    assert_eq!(provider_bin(HarnessProvider::Claude, &e), "/opt/claude");
    // Whitespace-only override falls back to the default.
    let e = env(&[("TINYPLACE_CODEX_BIN", "   ")]);
    assert_eq!(provider_bin(HarnessProvider::Codex, &e), "codex");
}

#[test]
fn provider_args_whitespace_split() {
    assert!(provider_args(HarnessProvider::Codex, &env(&[])).is_empty());
    let e = env(&[("TINYPLACE_CODEX_ARGS", "  --foo   bar --baz ")]);
    assert_eq!(
        provider_args(HarnessProvider::Codex, &e),
        vec!["--foo", "bar", "--baz"]
    );
    // A different provider's args do not leak.
    assert!(provider_args(HarnessProvider::Claude, &e).is_empty());
}

#[test]
fn sessions_dir_precedence() {
    // Per-provider beats TINYVERSE beats HARNESS.
    let e = env(&[
        ("TINYPLACE_CLAUDE_SESSIONS_DIR", "/p"),
        ("TINYVERSE_CLAUDE_SESSIONS_DIR", "/tv"),
        ("TINYPLACE_HARNESS_SESSIONS_DIR", "/h"),
    ]);
    assert_eq!(
        sessions_dir(HarnessProvider::Claude, &e),
        PathBuf::from("/p")
    );

    let e = env(&[
        ("TINYVERSE_CLAUDE_SESSIONS_DIR", "/tv"),
        ("TINYPLACE_HARNESS_SESSIONS_DIR", "/h"),
    ]);
    assert_eq!(
        sessions_dir(HarnessProvider::Claude, &e),
        PathBuf::from("/tv")
    );

    // TINYVERSE is claude-only; codex ignores it and uses HARNESS.
    let e = env(&[
        ("TINYVERSE_CLAUDE_SESSIONS_DIR", "/tv"),
        ("TINYPLACE_HARNESS_SESSIONS_DIR", "/h"),
    ]);
    assert_eq!(
        sessions_dir(HarnessProvider::Codex, &e),
        PathBuf::from("/h")
    );

    // Default when nothing set (ends with the provider-specific suffix).
    assert!(sessions_dir(HarnessProvider::Codex, &env(&[])).ends_with("sessions"));
    assert!(sessions_dir(HarnessProvider::Claude, &env(&[])).ends_with("projects"));
}

#[test]
fn timings_defaults_and_numeric_fallback() {
    let empty = env(&[]);
    assert_eq!(session_poll_ms(HarnessProvider::Codex, &empty), 500);
    assert_eq!(receive_poll_ms(HarnessProvider::Codex, &empty), 1_500);
    assert_eq!(status_heartbeat_ms(HarnessProvider::Codex, &empty), 15_000);
    assert_eq!(status_idle_ms(HarnessProvider::Codex, &empty), 30_000);

    // Per-provider beats generic.
    let e = env(&[
        ("TINYPLACE_HARNESS_SESSION_POLL_MS", "800"),
        ("TINYPLACE_CODEX_SESSION_POLL_MS", "250"),
    ]);
    assert_eq!(session_poll_ms(HarnessProvider::Codex, &e), 250);
    // Generic applies when no per-provider key.
    assert_eq!(session_poll_ms(HarnessProvider::Claude, &e), 800);

    // Non-numeric / zero / negative → default silently.
    for bad in ["abc", "0", "-5", "  "] {
        let e = env(&[("TINYPLACE_CODEX_RECEIVE_POLL_MS", bad)]);
        assert_eq!(receive_poll_ms(HarnessProvider::Codex, &e), 1_500);
    }
    // Whitespace-padded numeric parses.
    let e = env(&[("TINYPLACE_CODEX_STATUS_IDLE_MS", " 12345 ")]);
    assert_eq!(status_idle_ms(HarnessProvider::Codex, &e), 12_345);
}

/// Deserialize a `RouterConfig` from a JSON literal for the resolver tests.
fn router(json: &str) -> RouterConfig {
    serde_json::from_str(json).expect("valid router config")
}

#[test]
fn router_env_no_config_is_empty_for_every_provider() {
    // An empty router (no baseUrl anywhere) injects nothing — the child spawns
    // exactly as it would with no [router] section at all.
    let cfg = RouterConfig::default();
    for provider in [
        HarnessProvider::Claude,
        HarnessProvider::Codex,
        HarnessProvider::Opencode,
    ] {
        let injection = router_env(provider, &cfg);
        assert!(injection.is_empty(), "{provider:?} must inject nothing");
        assert!(injection.env.is_empty());
        assert!(injection.secret_env.is_empty());
        assert!(injection.args.is_empty());
    }
}

#[test]
fn router_env_codex_emits_openai_base_and_key_by_name() {
    let cfg = router(r#"{"baseUrl":"https://gw/v1","apiKeyEnv":"MEDULLA_ROUTER_KEY"}"#);
    let injection = router_env(HarnessProvider::Codex, &cfg);
    assert_eq!(
        injection.env,
        vec![("OPENAI_BASE_URL".to_string(), "https://gw/v1".to_string())]
    );
    // The key is referenced by env-var NAME, never the value.
    assert_eq!(
        injection.secret_env,
        vec![(
            "OPENAI_API_KEY".to_string(),
            "MEDULLA_ROUTER_KEY".to_string()
        )]
    );
    assert!(injection.args.is_empty());
}

#[test]
fn router_env_opencode_uses_openai_compatible_env() {
    let cfg = router(r#"{"baseUrl":"https://gw/v1","apiKeyEnv":"OC_KEY"}"#);
    let injection = router_env(HarnessProvider::Opencode, &cfg);
    assert_eq!(
        injection.env,
        vec![("OPENAI_BASE_URL".to_string(), "https://gw/v1".to_string())]
    );
    assert_eq!(
        injection.secret_env,
        vec![("OPENAI_API_KEY".to_string(), "OC_KEY".to_string())]
    );
}

#[test]
fn router_env_claude_emits_anthropic_base_and_auth_token() {
    // Claude speaks the Anthropic wire format: base URL + AUTH_TOKEN (by name).
    let cfg = router(r#"{"baseUrl":"https://gw/anthropic","apiKeyEnv":"MEDULLA_ROUTER_KEY"}"#);
    let injection = router_env(HarnessProvider::Claude, &cfg);
    assert_eq!(
        injection.env,
        vec![(
            "ANTHROPIC_BASE_URL".to_string(),
            "https://gw/anthropic".to_string()
        )]
    );
    assert_eq!(
        injection.secret_env,
        vec![(
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            "MEDULLA_ROUTER_KEY".to_string()
        )]
    );
}

#[test]
fn router_env_provider_override_beats_top_level() {
    // providers.claude.baseUrl (Anthropic-passthrough) wins for claude, while
    // codex inherits the top-level OpenAI-compatible endpoint.
    let cfg = router(
        r#"{
            "baseUrl":"https://top/v1",
            "apiKeyEnv":"K",
            "providers":{"claude":{"baseUrl":"https://gw/anthropic"}}
        }"#,
    );
    let claude = router_env(HarnessProvider::Claude, &cfg);
    assert_eq!(
        claude.env,
        vec![(
            "ANTHROPIC_BASE_URL".to_string(),
            "https://gw/anthropic".to_string()
        )]
    );
    let codex = router_env(HarnessProvider::Codex, &cfg);
    assert_eq!(
        codex.env,
        vec![("OPENAI_BASE_URL".to_string(), "https://top/v1".to_string())]
    );
}

#[test]
fn router_env_without_api_key_env_injects_endpoint_only() {
    // A router with an endpoint but no apiKeyEnv steers the base URL and leaves
    // the harness's own credentials in place (no secret_env binding).
    let cfg = router(r#"{"baseUrl":"https://gw/v1"}"#);
    let injection = router_env(HarnessProvider::Codex, &cfg);
    assert_eq!(
        injection.env,
        vec![("OPENAI_BASE_URL".to_string(), "https://gw/v1".to_string())]
    );
    assert!(
        injection.secret_env.is_empty(),
        "no apiKeyEnv → no key binding"
    );

    // An empty apiKeyEnv name is treated as unset.
    let blank = router(r#"{"baseUrl":"https://gw/v1","apiKeyEnv":""}"#);
    assert!(router_env(HarnessProvider::Codex, &blank)
        .secret_env
        .is_empty());
}

#[test]
fn router_env_key_without_base_url_injects_nothing() {
    // apiKeyEnv set but no baseUrl (for this provider) → the router is not
    // routing here, so nothing is injected and the child keeps its own endpoint.
    let cfg = router(r#"{"apiKeyEnv":"K"}"#);
    assert!(router_env(HarnessProvider::Codex, &cfg).is_empty());
}
