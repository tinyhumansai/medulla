//! Backend bearer-token resolution: pick the effective token from config, the
//! environment, or stored credentials; describe the missing-token state; and
//! classify one-time login tokens versus JWTs. Depends on
//! [`crate::config::BackendConfig`] for the configured backend.

use std::collections::HashMap;

use super::types::Credentials;

/// Resolve the backend bearer token from, in precedence order:
///
/// 1. an inline `backend.token` in the loaded config,
/// 2. the `backend.tokenEnv` environment variable (an empty value is ignored),
/// 3. `stored` credentials saved by `medulla login` — but only when their
///    `baseUrl` matches the configured backend after trailing-slash
///    normalization (a mismatch is ignored so credentials for one backend never
///    leak to another).
///
/// Returns `None` when no source yields a token. Pure over its inputs; the caller
/// supplies the process environment and any stored credentials.
pub fn resolve_backend_token(
    env: &HashMap<String, String>,
    backend: &crate::config::BackendConfig,
    stored: Option<&Credentials>,
) -> Option<String> {
    if let Some(tok) = backend.token.clone() {
        return Some(tok);
    }
    if let Some(tok) = env
        .get(&backend.token_env)
        .cloned()
        .filter(|s| !s.is_empty())
    {
        return Some(tok);
    }
    let want = backend.base_url.trim_end_matches('/');
    stored
        .filter(|c| c.base_url.trim_end_matches('/') == want)
        .map(|c| c.jwt.clone())
}

/// The status note shown when no backend token is available and the mock runs.
///
/// Names the environment variable the operator can set to supply a token.
pub fn missing_token_note(backend: &crate::config::BackendConfig) -> String {
    format!(
        "backend token missing (set ${} or run `medulla login`) — running with mock runtime",
        backend.token_env
    )
}

/// Whether `s` looks like a one-time login token (64 lowercase hex characters)
/// rather than a JWT.
///
/// The backend issues these short-lived tokens from the login page; a caller
/// redeems one via [`crate::client::MedullaClient::consume_login_token`], whereas
/// a value that fails this check is treated as a ready-to-use JWT. Centralizes
/// the format contract so every front end classifies login input identically.
pub fn is_one_time_login_token(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}
