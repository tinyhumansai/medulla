//! Backend bearer-token resolution: pick the effective token from config, the
//! environment, or the stored app session; describe the missing-token state; and
//! classify one-time login tokens versus JWTs. Depends on
//! [`crate::config::BackendConfig`] for the configured backend.

use std::collections::HashMap;

/// Resolve the backend bearer token from, in precedence order:
///
/// 1. an inline `backend.token` in the loaded config,
/// 2. the `backend.tokenEnv` environment variable (an empty value is ignored),
/// 3. `session` — the stored app session, read by the caller via
///    [`crate::auth::session_token`].
///
/// Returns `None` when no source yields a token. Pure over its inputs; the caller
/// supplies the process environment and the session.
///
/// # Why the session is not matched against `backend.base_url`
///
/// It used to be, and a URL comparison rejected perfectly good tokens whenever
/// the two spellings drifted — a trailing slash was enough, which is a worse
/// failure than the mismatch it guarded against. The store records the issuing
/// origin for diagnostics ([`crate::auth::StoredSession::base_url`]) and leaves
/// the decision to the call that fails with a 401.
/// Whether a configured or environment token (rather than the stored
/// session) is what authenticates against `backend`.
///
/// Asks [`resolve_backend_token`] itself with no session, so it cannot drift
/// from the real precedence: an inline `backend.token` wins even when empty.
/// Telemetry uses this to avoid attributing a run to the stored account when
/// some other credential is the one actually in use.
pub fn external_token_wins(
    env: &HashMap<String, String>,
    backend: &crate::config::BackendConfig,
) -> bool {
    resolve_backend_token(env, backend, None).is_some()
}

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
