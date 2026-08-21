//! Backend bearer-token resolution: pick the effective token from config, the
//! environment, or the core's app session; describe the missing-token state; and
//! classify one-time login tokens versus JWTs. Depends on
//! [`crate::config::BackendConfig`] for the configured backend.

use std::collections::HashMap;

/// Resolve the backend bearer token from, in precedence order:
///
/// 1. an inline `backend.token` in the loaded config,
/// 2. the `backend.tokenEnv` environment variable (an empty value is ignored),
/// 3. `session` — the app session the embedded core holds, read by the caller
///    via the core's typed auth facade (`core.auth().token()`).
///
/// Returns `None` when no source yields a token. Pure over its inputs; the caller
/// supplies the process environment and the session.
///
/// # Why the session is no longer matched against `backend.base_url`
///
/// It used to be, because a Medulla-owned credential file could hold a JWT for
/// one deployment while the config named another. There is now one session, held
/// by the core, for the deployment the core itself resolves — a URL comparison
/// against a *different* config value would reject a perfectly good token
/// whenever the two spellings drifted, which is a worse failure than the one it
/// guarded against.
pub fn resolve_backend_token(
    env: &HashMap<String, String>,
    backend: &crate::config::BackendConfig,
    session: Option<&str>,
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
    session.map(str::to_string).filter(|t| !t.trim().is_empty())
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
