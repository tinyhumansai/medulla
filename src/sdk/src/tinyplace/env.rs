//! Centralized environment-variable resolution for the harness wrapper and the
//! headless daemon.
//!
//! Every knob the wrapper/daemon read from the process environment resolves
//! here, as a pure function over an injected `&HashMap<String, String>` so the
//! precedence matrix is unit-testable and identical across both call sites. The
//! contract mirrors the TypeScript reference wrapper
//! (`vendor/tinyplace/sdk/typescript/src/cli/harness-wrapper.ts`): a
//! per-provider key always beats the generic (`HARNESS`) key, which beats the
//! owner fallbacks / provider defaults.
//!
//! `<P>` is the uppercased provider (`CODEX` / `CLAUDE` / `OPENCODE`).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::tinyplace::HarnessProvider;

/// Default wrapper session-file poll interval (ms).
pub const DEFAULT_SESSION_POLL_MS: u64 = 500;
/// Default inbound-receive poll interval (ms).
pub const DEFAULT_RECEIVE_POLL_MS: u64 = 1_500;
/// Default status-heartbeat re-emit interval (ms).
pub const DEFAULT_STATUS_HEARTBEAT_MS: u64 = 15_000;
/// Default silence-before-idle interval (ms).
pub const DEFAULT_STATUS_IDLE_MS: u64 = 30_000;

/// The first non-empty value among `keys`, in order.
fn first_env<'a>(env: &'a HashMap<String, String>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .filter(|key| !key.is_empty())
        .filter_map(|key| env.get(*key))
        .map(String::as_str)
        .find(|value| !value.is_empty())
}

/// `TINYPLACE_<P>_<SUFFIX>` for the given provider.
fn provider_key(provider: HarnessProvider, suffix: &str) -> String {
    format!("TINYPLACE_{}_{suffix}", provider.as_str().to_uppercase())
}

/// The owner this session forwards envelopes to (and, by default, receives input
/// from). Order: `TINYPLACE_<P>_DM_TO` > `TINYPLACE_HARNESS_DM_TO` >
/// `TINYPLACE_OPENHUMAN_OWNER` > `OPENHUMAN_OWNER_AGENT`.
pub fn dm_recipient(provider: HarnessProvider, env: &HashMap<String, String>) -> Option<String> {
    first_env(
        env,
        &[
            &provider_key(provider, "DM_TO"),
            "TINYPLACE_HARNESS_DM_TO",
            "TINYPLACE_OPENHUMAN_OWNER",
            "OPENHUMAN_OWNER_AGENT",
        ],
    )
    .map(str::to_string)
}

/// The peer whose inbound frames / plain DMs are injected as input. Order:
/// `TINYPLACE_<P>_RECEIVE_FROM` > `TINYPLACE_HARNESS_RECEIVE_FROM`, then falls
/// back to the DM recipient.
pub fn receive_from(
    provider: HarnessProvider,
    env: &HashMap<String, String>,
    recipient: Option<&str>,
) -> Option<String> {
    first_env(
        env,
        &[
            &provider_key(provider, "RECEIVE_FROM"),
            "TINYPLACE_HARNESS_RECEIVE_FROM",
        ],
    )
    .map(str::to_string)
    .or_else(|| recipient.map(str::to_string))
}

/// Inbound input is enabled unless `TINYPLACE_<P>_RECEIVE` /
/// `TINYPLACE_HARNESS_RECEIVE` is set to `"0"` (per-provider beats generic).
pub fn receive_enabled(provider: HarnessProvider, env: &HashMap<String, String>) -> bool {
    for key in [
        provider_key(provider, "RECEIVE"),
        "TINYPLACE_HARNESS_RECEIVE".to_string(),
    ] {
        if let Some(value) = env.get(&key) {
            if !value.is_empty() {
                return value != "0";
            }
        }
    }
    true
}

/// Per-provider binary override keys (first non-empty wins), highest precedence
/// first. Claude also honors the legacy `TINYVERSE_CLAUDE_BIN`.
fn bin_keys(provider: HarnessProvider) -> &'static [&'static str] {
    match provider {
        HarnessProvider::Claude => &["TINYVERSE_CLAUDE_BIN", "TINYPLACE_CLAUDE_BIN"],
        HarnessProvider::Codex => &["TINYPLACE_CODEX_BIN"],
        HarnessProvider::Opencode => &["TINYPLACE_OPENCODE_BIN"],
    }
}

fn default_bin(provider: HarnessProvider) -> &'static str {
    provider.as_str()
}

/// Resolve the provider binary: the first non-empty override, else the default
/// (`claude` / `codex` / `opencode`). Overrides are trimmed.
pub fn provider_bin(provider: HarnessProvider, env: &HashMap<String, String>) -> String {
    for key in bin_keys(provider) {
        if let Some(value) = env.get(*key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    default_bin(provider).to_string()
}

/// Extra args prepended to the child argv, from `TINYPLACE_<P>_ARGS`
/// (whitespace-split). Empty / unset yields no args.
pub fn provider_args(provider: HarnessProvider, env: &HashMap<String, String>) -> Vec<String> {
    match env.get(&provider_key(provider, "ARGS")) {
        Some(raw) => raw.split_whitespace().map(str::to_string).collect(),
        None => Vec::new(),
    }
}

/// The provider's default session-transcript directory.
fn default_sessions_dir(provider: HarnessProvider) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    match provider {
        HarnessProvider::Claude => home.join(".claude").join("projects"),
        HarnessProvider::Codex => home.join(".codex").join("sessions"),
        HarnessProvider::Opencode => home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("sessions"),
    }
}

/// Resolve the session-transcript directory. Order:
/// `TINYPLACE_<P>_SESSIONS_DIR` > (claude only) `TINYVERSE_CLAUDE_SESSIONS_DIR` >
/// `TINYPLACE_HARNESS_SESSIONS_DIR` > provider default.
pub fn sessions_dir(provider: HarnessProvider, env: &HashMap<String, String>) -> PathBuf {
    let provider_dir = provider_key(provider, "SESSIONS_DIR");
    let tinyverse = if provider == HarnessProvider::Claude {
        "TINYVERSE_CLAUDE_SESSIONS_DIR"
    } else {
        ""
    };
    first_env(
        env,
        &[&provider_dir, tinyverse, "TINYPLACE_HARNESS_SESSIONS_DIR"],
    )
    .map(PathBuf::from)
    .unwrap_or_else(|| default_sessions_dir(provider))
}

/// Parse a positive-numeric env value, falling back silently on unset,
/// non-numeric, zero, or negative values (TS `numberEnvOr` parity).
fn number_env_or(raw: Option<&str>, fallback: u64) -> u64 {
    match raw {
        Some(value) => match value.trim().parse::<i64>() {
            Ok(parsed) if parsed > 0 => parsed as u64,
            _ => fallback,
        },
        None => fallback,
    }
}

fn timing(
    provider: HarnessProvider,
    env: &HashMap<String, String>,
    suffix: &str,
    fallback: u64,
) -> u64 {
    let provider_specific = provider_key(provider, suffix);
    let generic = format!("TINYPLACE_HARNESS_{suffix}");
    number_env_or(first_env(env, &[&provider_specific, &generic]), fallback)
}

/// `TINYPLACE_<P>_SESSION_POLL_MS` / `TINYPLACE_HARNESS_SESSION_POLL_MS` (500).
pub fn session_poll_ms(provider: HarnessProvider, env: &HashMap<String, String>) -> u64 {
    timing(provider, env, "SESSION_POLL_MS", DEFAULT_SESSION_POLL_MS)
}

/// `TINYPLACE_<P>_RECEIVE_POLL_MS` / `TINYPLACE_HARNESS_RECEIVE_POLL_MS` (1500).
pub fn receive_poll_ms(provider: HarnessProvider, env: &HashMap<String, String>) -> u64 {
    timing(provider, env, "RECEIVE_POLL_MS", DEFAULT_RECEIVE_POLL_MS)
}

/// `TINYPLACE_<P>_STATUS_HEARTBEAT_MS` / `TINYPLACE_HARNESS_STATUS_HEARTBEAT_MS`
/// (15000).
pub fn status_heartbeat_ms(provider: HarnessProvider, env: &HashMap<String, String>) -> u64 {
    timing(
        provider,
        env,
        "STATUS_HEARTBEAT_MS",
        DEFAULT_STATUS_HEARTBEAT_MS,
    )
}

/// `TINYPLACE_<P>_STATUS_IDLE_MS` / `TINYPLACE_HARNESS_STATUS_IDLE_MS` (30000).
pub fn status_idle_ms(provider: HarnessProvider, env: &HashMap<String, String>) -> u64 {
    timing(provider, env, "STATUS_IDLE_MS", DEFAULT_STATUS_IDLE_MS)
}

#[cfg(test)]
mod tests {
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
}
