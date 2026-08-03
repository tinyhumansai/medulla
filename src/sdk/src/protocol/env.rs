//! Centralized environment-variable resolution for the harness wrapper and the
//! headless daemon.
//!
//! Every knob the wrapper/daemon read from the process environment resolves
//! here, as a pure function over an injected `&HashMap<String, String>` so the
//! precedence matrix is unit-testable and identical across both call sites. The
//! contract follows the public tiny.place wrapper specification: a
//! per-provider key always beats the generic (`HARNESS`) key, which beats the
//! owner fallbacks / provider defaults.
//!
//! `<P>` is the uppercased provider (`CODEX` / `CLAUDE` / `OPENCODE`).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::RouterConfig;
use crate::protocol::HarnessProvider;

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
        HarnessProvider::Openhuman => &["OPENHUMAN_BIN"],
    }
}

fn default_bin(provider: HarnessProvider) -> &'static str {
    match provider {
        // The crate's installed binary retains this historical name even
        // though the product and picker label are simply OpenHuman.
        HarnessProvider::Openhuman => "openhuman-core",
        _ => provider.as_str(),
    }
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

/// What the router injects into one provider's spawn environment.
///
/// Produced by [`router_env`] as a pure function of the [`RouterConfig`] and the
/// target provider — it performs no I/O and never reads a secret. The API key is
/// carried as a *name* to resolve, never a value: [`secret_env`](Self::secret_env)
/// maps a child env var to the name of the daemon env var holding the key, and
/// the spawn layer resolves it at launch. Everything here is safe to log except
/// the resolved key, which never enters this struct.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouterInjection {
    /// Literal env vars to set on the child (e.g. `OPENAI_BASE_URL`,
    /// `ANTHROPIC_BASE_URL`) — the endpoint, never a secret.
    pub env: Vec<(String, String)>,
    /// `(child_var, source_env_name)`: the child's key variable and the name of
    /// the daemon env var whose value must be copied into it at spawn. The value
    /// is resolved by the spawn layer, never here.
    pub secret_env: Vec<(String, String)>,
    /// Extra CLI args to prepend to the child argv. Empty for the env-based
    /// (`*_BASE_URL`) mechanism used today; reserved for the codex
    /// `--config model_provider=<id>` route, whose on-disk `model_providers`
    /// block is out of scope for this repo (IP boundary: env injection only).
    pub args: Vec<String>,
}

impl RouterInjection {
    /// Whether the router leaves this provider's environment untouched — i.e.
    /// nothing to inject (no effective endpoint configured).
    pub fn is_empty(&self) -> bool {
        self.env.is_empty() && self.secret_env.is_empty() && self.args.is_empty()
    }
}

/// The child env var names a provider uses to reach a custom endpoint:
/// `(base_url_var, api_key_var)`.
///
/// - claude speaks the Anthropic wire format: `ANTHROPIC_BASE_URL` +
///   `ANTHROPIC_AUTH_TOKEN` (an OpenAI-compatible endpoint needs a translating
///   proxy, configured as the provider's Anthropic-passthrough `baseUrl`).
/// - codex and opencode are natively OpenAI-compatible: `OPENAI_BASE_URL` +
///   `OPENAI_API_KEY`. opencode's native "provider block" mechanism is realized
///   here through its OpenAI-compatible env, since writing the harness's on-disk
///   config is out of scope (IP boundary: env injection at the spawn seam only).
fn router_env_vars(provider: HarnessProvider) -> (&'static str, &'static str) {
    match provider {
        HarnessProvider::Claude => ("ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN"),
        HarnessProvider::Codex => ("OPENAI_BASE_URL", "OPENAI_API_KEY"),
        HarnessProvider::Opencode => ("OPENAI_BASE_URL", "OPENAI_API_KEY"),
        // OpenHuman uses the shared core's own provider/agent configuration;
        // it is not redirected through a coding-harness router.
        HarnessProvider::Openhuman => ("", ""),
    }
}

/// Resolve, purely, what a `RouterConfig` injects into `provider`'s spawn
/// environment.
///
/// Precedence for the endpoint is `providers.<p>.baseUrl`, then top-level
/// `baseUrl`, then unset (the harness's own on-disk config), via
/// [`RouterConfig::base_url_for`]. The key is bound only when the provider has an
/// effective endpoint *and* an `apiKeyEnv` name is configured: without an
/// endpoint the router is not routing this provider anywhere, so swapping its
/// credentials would be wrong. When there is no endpoint the result is empty and
/// the child spawns exactly as it would with no `[router]` at all.
///
/// This function is side-effect-free: it never reads `process.env`, never reads
/// the daemon environment, and never returns the API key value — only its
/// env-var name, for the spawn layer to resolve.
pub fn router_env(provider: HarnessProvider, router: &RouterConfig) -> RouterInjection {
    let mut injection = RouterInjection::default();
    if provider == HarnessProvider::Openhuman {
        return injection;
    }
    let Some(base_url) = router.base_url_for(provider.as_str()) else {
        // No endpoint for this provider → nothing to inject (feature off here).
        return injection;
    };
    let (base_var, key_var) = router_env_vars(provider);
    injection
        .env
        .push((base_var.to_string(), base_url.to_string()));
    // Bind the key by NAME, never by value. Skipped when unset/empty so the
    // harness keeps its own credentials against the routed endpoint.
    if let Some(key_env) = router
        .api_key_env
        .as_deref()
        .filter(|name| !name.is_empty())
    {
        injection
            .secret_env
            .push((key_var.to_string(), key_env.to_string()));
    }
    injection
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
        HarnessProvider::Openhuman => home.join(".openhuman"),
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
#[path = "env_tests.rs"]
mod tests;
