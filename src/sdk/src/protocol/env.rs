//! Centralized environment-variable resolution for the harness wrapper and the
//! headless daemon.
//!
//! Every knob the wrapper/daemon read from the process environment resolves
//! here, as a pure function over an injected `&HashMap<String, String>` so the
//! precedence matrix is unit-testable and identical across both call sites.
//!
//! Precedence: a per-provider key always beats the generic (`HARNESS`) key,
//! which beats the owner fallbacks / provider defaults. Within each tier the
//! `MEDULLA_*` name wins and the deprecated `TINYPLACE_*` spelling is read
//! directly behind it, so hosts configured before the rename keep working.
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

/// The per-provider keys for `suffix`, highest precedence first:
/// `MEDULLA_<P>_<SUFFIX>` then the deprecated `TINYPLACE_<P>_<SUFFIX>`.
///
/// Both namespaces are read because the `TINYPLACE_*` names are what deployed
/// hosts and shell profiles already set. Dropping them would not fail loudly —
/// a worker whose `TINYPLACE_HARNESS_DM_TO` stopped resolving simply runs as a
/// plain passthrough, owning nothing and serving nobody, which reads as the
/// harness being broken rather than as a config that needs renaming.
fn provider_keys(provider: HarnessProvider, suffix: &str) -> [String; 2] {
    let p = provider.as_str().to_uppercase();
    [
        format!("MEDULLA_{p}_{suffix}"),
        format!("TINYPLACE_{p}_{suffix}"),
    ]
}

/// The generic (non-provider) keys for `suffix`, highest precedence first:
/// `MEDULLA_HARNESS_<SUFFIX>` then the deprecated `TINYPLACE_HARNESS_<SUFFIX>`.
fn harness_keys(suffix: &str) -> [String; 2] {
    [
        format!("MEDULLA_HARNESS_{suffix}"),
        format!("TINYPLACE_HARNESS_{suffix}"),
    ]
}

/// The owner this session forwards envelopes to (and, by default, receives input
/// from). Order: `MEDULLA_<P>_DM_TO` > `MEDULLA_HARNESS_DM_TO` >
/// `MEDULLA_OPENHUMAN_OWNER` > `OPENHUMAN_OWNER_AGENT`, with the deprecated
/// `TINYPLACE_*` spelling of each read directly behind it.
pub fn dm_recipient(provider: HarnessProvider, env: &HashMap<String, String>) -> Option<String> {
    let provider_keys = provider_keys(provider, "DM_TO");
    let harness_keys = harness_keys("DM_TO");
    first_env(
        env,
        &[
            &provider_keys[0],
            &provider_keys[1],
            &harness_keys[0],
            &harness_keys[1],
            "MEDULLA_OPENHUMAN_OWNER",
            "TINYPLACE_OPENHUMAN_OWNER",
            "OPENHUMAN_OWNER_AGENT",
        ],
    )
    .map(str::to_string)
}

/// The peer whose inbound frames / plain DMs are injected as input. Order:
/// `MEDULLA_<P>_RECEIVE_FROM` > `MEDULLA_HARNESS_RECEIVE_FROM` (each followed by
/// its deprecated `TINYPLACE_*` spelling), then falls back to the DM recipient.
pub fn receive_from(
    provider: HarnessProvider,
    env: &HashMap<String, String>,
    recipient: Option<&str>,
) -> Option<String> {
    let provider_keys = provider_keys(provider, "RECEIVE_FROM");
    let harness_keys = harness_keys("RECEIVE_FROM");
    first_env(
        env,
        &[
            &provider_keys[0],
            &provider_keys[1],
            &harness_keys[0],
            &harness_keys[1],
        ],
    )
    .map(str::to_string)
    .or_else(|| recipient.map(str::to_string))
}

/// Inbound input is enabled unless `MEDULLA_<P>_RECEIVE` /
/// `MEDULLA_HARNESS_RECEIVE` (or their deprecated `TINYPLACE_*` spellings) is
/// set to `"0"` (per-provider beats generic).
pub fn receive_enabled(provider: HarnessProvider, env: &HashMap<String, String>) -> bool {
    let provider_keys = provider_keys(provider, "RECEIVE");
    let harness_keys = harness_keys("RECEIVE");
    for key in provider_keys.iter().chain(harness_keys.iter()) {
        if let Some(value) = env.get(key) {
            if !value.is_empty() {
                return value != "0";
            }
        }
    }
    true
}

/// Per-provider binary override keys (first non-empty wins), highest precedence
/// first. Claude also honors the legacy `TINYVERSE_CLAUDE_BIN`; the
/// `MEDULLA_*` spellings are deprecated but still read.
fn bin_keys(provider: HarnessProvider) -> &'static [&'static str] {
    match provider {
        HarnessProvider::Claude => &[
            "MEDULLA_CLAUDE_BIN",
            "TINYVERSE_CLAUDE_BIN",
            "TINYPLACE_CLAUDE_BIN",
        ],
        HarnessProvider::Codex => &["MEDULLA_CODEX_BIN", "TINYPLACE_CODEX_BIN"],
        HarnessProvider::Opencode => &["MEDULLA_OPENCODE_BIN", "TINYPLACE_OPENCODE_BIN"],
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

/// Whether `bin` is something other than `provider`'s verified default
/// (`claude` / `codex` / `opencode` / `openhuman-core`).
///
/// This is what lets [`crate::mcp::attach_cli`] withhold the fleet grant from
/// an overridden binary rather than hand it out: that binary is whatever the
/// override names, executed as the harness `--mcp-config` registers Medulla's
/// tools onto — so it receives that registration's argv itself and can open
/// the file it names, same-user permissions notwithstanding, regardless of
/// whether it behaves like the real Claude Code CLI once it does. Repository
/// policy already calls a provider-binary override untrusted configuration to
/// be validated at a boundary (see `AGENTS.md`'s security section); this is
/// that boundary for the one credential this project hands a CLI-spawned
/// harness.
///
/// # What a name-only check does not cover
///
/// The comparison is on the resolved *name*, so the bare default (`claude`)
/// is trusted, and the spawn resolves that name through `PATH`. An attacker
/// who could write `PATH` into a harness spawn's environment could therefore
/// point it at their own `claude` while this reports "not overridden".
///
/// That is a stated limit rather than a live hole: no configuration surface
/// writes `PATH` into a spawn environment. The only writes are fixed-key ones
/// — the `[router]` injection, attribution, a custom harness preset's
/// `ANTHROPIC_*` models, and the MCP variables — and the child's `PATH` is
/// inherited from Medulla's own process. Anyone who can change *that* `PATH`
/// is already executing as the operator, which is strictly more than the
/// grant is worth. If a config surface ever gains a general environment map,
/// this check has to resolve the binary to an absolute path before comparing,
/// and this paragraph is the reason why.
///
/// Takes the **already-resolved** binary rather than an environment to resolve
/// one from, and that is the whole point of the signature. A caller can hold
/// more than one environment — `PtySessionExecutor` selects the executable
/// from its own `self.env` while handing the child a per-run environment
/// derived separately — and a trust decision that re-derived the binary from
/// the *other* one would clear a wrapper that is about to be launched. There
/// is only one executable, so only the executable is asked about.
pub fn bin_is_overridden(provider: HarnessProvider, bin: &str) -> bool {
    bin.trim() != default_bin(provider)
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

/// Extra args prepended to the child argv, from `MEDULLA_<P>_ARGS` (or the
/// deprecated `TINYPLACE_<P>_ARGS`), whitespace-split. Empty / unset yields no
/// args.
pub fn provider_args(provider: HarnessProvider, env: &HashMap<String, String>) -> Vec<String> {
    let keys = provider_keys(provider, "ARGS");
    match first_env(env, &[&keys[0], &keys[1]]) {
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
/// `MEDULLA_<P>_SESSIONS_DIR` > (claude only) `TINYVERSE_CLAUDE_SESSIONS_DIR` >
/// `MEDULLA_HARNESS_SESSIONS_DIR` > provider default, with the deprecated
/// `TINYPLACE_*` spelling of each read directly behind it.
pub fn sessions_dir(provider: HarnessProvider, env: &HashMap<String, String>) -> PathBuf {
    let provider_dirs = provider_keys(provider, "SESSIONS_DIR");
    let harness_dirs = harness_keys("SESSIONS_DIR");
    let tinyverse = if provider == HarnessProvider::Claude {
        "TINYVERSE_CLAUDE_SESSIONS_DIR"
    } else {
        ""
    };
    first_env(
        env,
        &[
            &provider_dirs[0],
            &provider_dirs[1],
            tinyverse,
            &harness_dirs[0],
            &harness_dirs[1],
        ],
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
    let provider_specific = provider_keys(provider, suffix);
    let generic = harness_keys(suffix);
    number_env_or(
        first_env(
            env,
            &[
                &provider_specific[0],
                &provider_specific[1],
                &generic[0],
                &generic[1],
            ],
        ),
        fallback,
    )
}

/// `MEDULLA_<P>_SESSION_POLL_MS` / `MEDULLA_HARNESS_SESSION_POLL_MS` (500).
pub fn session_poll_ms(provider: HarnessProvider, env: &HashMap<String, String>) -> u64 {
    timing(provider, env, "SESSION_POLL_MS", DEFAULT_SESSION_POLL_MS)
}

/// `MEDULLA_<P>_RECEIVE_POLL_MS` / `MEDULLA_HARNESS_RECEIVE_POLL_MS` (1500).
pub fn receive_poll_ms(provider: HarnessProvider, env: &HashMap<String, String>) -> u64 {
    timing(provider, env, "RECEIVE_POLL_MS", DEFAULT_RECEIVE_POLL_MS)
}

/// `MEDULLA_<P>_STATUS_HEARTBEAT_MS` / `MEDULLA_HARNESS_STATUS_HEARTBEAT_MS`
/// (15000).
pub fn status_heartbeat_ms(provider: HarnessProvider, env: &HashMap<String, String>) -> u64 {
    timing(
        provider,
        env,
        "STATUS_HEARTBEAT_MS",
        DEFAULT_STATUS_HEARTBEAT_MS,
    )
}

/// `MEDULLA_<P>_STATUS_IDLE_MS` / `MEDULLA_HARNESS_STATUS_IDLE_MS` (30000).
pub fn status_idle_ms(provider: HarnessProvider, env: &HashMap<String, String>) -> u64 {
    timing(provider, env, "STATUS_IDLE_MS", DEFAULT_STATUS_IDLE_MS)
}

#[cfg(test)]
#[path = "env_tests.rs"]
mod tests;
