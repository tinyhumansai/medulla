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
