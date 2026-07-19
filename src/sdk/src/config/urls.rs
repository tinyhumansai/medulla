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
/// Production tiny.place base URL (the default).
pub const PROD_TINYPLACE_BASE_URL: &str = "https://api.tiny.place";
/// Staging tiny.place base URL (selected by `MEDULLA_STAGING`).
pub const STAGING_TINYPLACE_BASE_URL: &str = "https://staging-api.tiny.place";

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

/// The default tiny.place base URL for this environment (staging vs prod).
pub fn default_tinyplace_base_url(env: &HashMap<String, String>) -> String {
    if is_staging(env) {
        STAGING_TINYPLACE_BASE_URL.to_string()
    } else {
        PROD_TINYPLACE_BASE_URL.to_string()
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

/// Resolve the tiny.place base URL for the `[tinyplace]` section. Order:
/// explicitly-configured `tinyplace.baseUrl` > staging/prod default. (The
/// `TINYPLACE_*`/`NEXT_PUBLIC_API_URL` env chain is applied later, at endpoint
/// resolution in [`crate::tinyplace::config`].)
pub fn resolve_tinyplace_base_url(
    env: &HashMap<String, String>,
    config_url: Option<&str>,
) -> String {
    if let Some(value) = config_url.map(str::trim).filter(|v| !v.is_empty()) {
        return value.to_string();
    }
    default_tinyplace_base_url(env)
}
