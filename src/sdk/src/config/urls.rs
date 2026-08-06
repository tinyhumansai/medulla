//! Endpoint base-URL constants and their environment-aware resolvers.
//!
//! Base-URL precedence cannot be expressed with serde defaults (which never see
//! the process environment), so it is resolved here as pure functions over an
//! injected `&HashMap<String, String>` and applied by
//! [`load_config`](super::load_config).

use std::collections::HashMap;

/// Production backend API base URL (the default).
pub const PROD_BACKEND_BASE_URL: &str = "https://api.tinyhumans.ai";
/// Staging backend API base URL (selected by `MEDULLA_STAGING`).
pub const STAGING_BACKEND_BASE_URL: &str = "https://staging-api.tinyhumans.ai";
// The link forwarder has no constants of its own: it is served by the same
// backend as everything else, so its default *follows* the resolved backend
// base URL rather than naming a second host. See
// [`resolve_forwarder_base_url`].

/// Whether `MEDULLA_STAGING` selects the staging defaults. Truthy is `"1"` or
/// `"true"` (case-insensitive, trimmed).
pub fn is_staging(env: &HashMap<String, String>) -> bool {
    env.get("MEDULLA_STAGING")
        .map(|v| crate::home::is_truthy(v))
        .unwrap_or(false)
}

/// The default backend base URL for this environment (staging vs prod).
pub fn default_backend_base_url(env: &HashMap<String, String>) -> String {
    if is_staging(env) {
        STAGING_BACKEND_BASE_URL.to_string()
    } else {
        PROD_BACKEND_BASE_URL.to_string()
    }
}

/// Resolve the backend base URL. Order: `MEDULLA_API_URL` env override >
/// explicitly-configured `backend.baseUrl` > staging/prod default. `config_url`
/// is the value present in the config file (`None` when the key was absent), so
/// an explicit config value is never clobbered by the default.
pub fn resolve_backend_base_url(env: &HashMap<String, String>, config_url: Option<&str>) -> String {
    if let Some(value) = env
        .get("MEDULLA_API_URL")
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        return value.to_string();
    }
    if let Some(value) = config_url.map(str::trim).filter(|v| !v.is_empty()) {
        return value.to_string();
    }
    default_backend_base_url(env)
}

/// The bare host of a base URL, for compact display in UI chrome.
///
/// Strips the scheme, any userinfo, the port, and any path/query/fragment, so
/// `https://api.tinyhumans.ai/v1` renders as `api.tinyhumans.ai`. Input that
/// does not parse as a URL is returned trimmed and unchanged rather than
/// erroring — this is display-only, and a malformed base URL is worth showing
/// verbatim so the user can spot the mistake.
pub fn display_host(base_url: &str) -> String {
    let trimmed = base_url.trim();
    let without_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let authority = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(without_scheme);
    // Drop userinfo (`user:pass@host`) and the port, keeping only the host.
    let host = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    // A bracketed IPv6 literal keeps its brackets; only a trailing `:port`
    // outside them is dropped.
    let host = match host.rfind(']') {
        Some(close) => &host[..=close],
        None => host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host),
    };
    if host.is_empty() {
        trimmed.to_string()
    } else {
        host.to_string()
    }
}

/// Resolve the link forwarder base URL for the `[link]` section. Order:
/// explicitly-configured `link.forwarderUrl` > the resolved backend base URL.
///
/// The forwarder is not a separate service on a separate host: the backend that
/// serves the API is the one that blindly forwards link datagrams, so pointing
/// `backend.baseUrl` (or `MEDULLA_API_URL`, or `MEDULLA_STAGING`) at a
/// deployment moves the forwarder with it. Deriving it rather than defaulting it
/// to a constant is what stops an operator who switched backends from being left
/// on the old deployment's forwarder — two endpoints on different forwarders both
/// start cleanly, report healthy, and never hear from each other.
///
/// `backend_url` is the already-resolved value from
/// [`resolve_backend_base_url`], so the two cannot drift.
pub fn resolve_forwarder_base_url(backend_url: &str, config_url: Option<&str>) -> String {
    if let Some(value) = config_url.map(str::trim).filter(|v| !v.is_empty()) {
        return value.to_string();
    }
    backend_url.to_string()
}
