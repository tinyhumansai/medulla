//! Building the backend client this runtime drives, and deciding whether a host
//! can actually use it.
//!
//! # Why readiness is three states, not two
//!
//! A host answers each differently: run, sign in, or stop. Collapsing the last
//! two would either send an operator to a login screen that cannot fix a missing
//! base URL, or refuse to start over a state one keystroke resolves.
//!
//! This replaces the probe that used to live in `core_host`, which asked a
//! booted OpenHuman core to list sessions and classified the `data.kind` on the
//! error it returned. Two of those kinds — "no session token", "no base URL" —
//! were the core reporting a decision this host had already made when it
//! configured the core, so the classification round-tripped through an RPC to
//! learn something already in hand. They are checked directly now, and only
//! genuine reachability costs a call.

use std::collections::HashMap;

use crate::client::MedullaClient;
use crate::config::BackendConfig;

/// Whether this host can reach a Medulla backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    /// The orchestration API answered — the backend is usable as a runtime.
    Ready,
    /// Reachable, but nobody is signed in. The host should run its login flow.
    SignedOut,
    /// There is no backend to talk to: no base URL configured. Carries an
    /// operator-safe message (no URLs, no tokens).
    Unusable(String),
}

/// Build the client this runtime drives from a loaded config and the process
/// environment.
///
/// The bearer comes from [`crate::auth::resolve_backend_token`], so an inline
/// `backend.token`, the `backend.tokenEnv` variable and the stored session are
/// consulted in that order — one precedence chain, shared with every other
/// backend-facing surface.
///
/// Returns `None` when the config names no backend or nothing yields a token;
/// use [`readiness`] to tell those two apart.
pub fn client_from_config(
    env: &HashMap<String, String>,
    backend: &BackendConfig,
) -> Option<MedullaClient> {
    let base_url = backend.base_url.trim();
    if base_url.is_empty() {
        return None;
    }
    let session = session_for(env, backend);
    let token = crate::auth::resolve_backend_token(env, backend, session.as_deref())?;
    Some(MedullaClient::new(base_url.to_string(), token))
}

/// The stored session, but only for the deployment that issued it.
///
/// # Why the issuer is checked here and nowhere else
///
/// `backend.baseUrl` is layered config, and the layers include
/// `<cwd>/.medulla/config.toml` and `<cwd>/medulla.toml` — files that come from
/// whatever repository the operator happens to be standing in. Handing the
/// account's stored bearer to whatever URL those name means launching Medulla
/// inside a hostile checkout discloses a production JWT to a host of the
/// attacker's choosing.
///
/// An inline `backend.token` or a `backend.tokenEnv` value is *not* filtered:
/// the operator put it there for this configuration, and it is theirs to aim.
/// The stored session is different — it was obtained for one deployment, and
/// nobody asked for it to be sent anywhere else.
///
/// [`StoredSession::base_url`](crate::auth::StoredSession) exists precisely for
/// this and was documented as "recorded for diagnostics, deliberately not
/// matched" — the reasoning being that an exact comparison rejected good tokens
/// over a trailing slash. That reasoning was about *spelling*, so the fix is a
/// normalized comparison rather than no comparison.
fn session_for(env: &HashMap<String, String>, backend: &BackendConfig) -> Option<String> {
    let stored = crate::auth::read(env)?;
    same_origin(&stored.base_url, &backend.base_url).then_some(stored.token)
}

/// Whether two backend URLs address the same deployment.
///
/// Compares scheme, host and port with the spelling differences that are not
/// differences removed: case in the scheme and host, a trailing slash, and a
/// port that is the scheme's default. Anything beyond the origin is ignored —
/// a path or query does not make it a different deployment.
///
/// A stored record with no origin at all (written before this field carried
/// one) does not match anything, so it is not sent anywhere. That is the safe
/// direction: the operator signs in again and the record gains its issuer.
fn same_origin(a: &str, b: &str) -> bool {
    fn parts(url: &str) -> Option<(String, String, Option<u16>)> {
        let url = url.trim().trim_end_matches('/');
        let (scheme, rest) = url.split_once("://")?;
        let scheme = scheme.to_ascii_lowercase();
        // Authority only: everything from the first `/`, `?` or `#` is path.
        let authority = rest
            .split(['/', '?', '#'])
            .next()
            .filter(|a| !a.is_empty())?;
        // Credentials in a URL are not part of its identity as a deployment.
        let authority = authority.rsplit('@').next()?;
        let (host, port) = match authority.rsplit_once(':') {
            // A `:` inside brackets is IPv6, not a port separator.
            Some((h, p)) if !h.ends_with(']') && p.chars().all(|c| c.is_ascii_digit()) => {
                (h, p.parse::<u16>().ok())
            }
            _ => (authority, None),
        };
        let default = match scheme.as_str() {
            "https" => Some(443),
            "http" => Some(80),
            _ => None,
        };
        let port = port.filter(|p| Some(*p) != default);
        Some((scheme, host.to_ascii_lowercase(), port))
    }
    match (parts(a), parts(b)) {
        (Some(a), Some(b)) => a == b,
        // An unparseable or absent origin matches nothing.
        _ => false,
    }
}

/// Classify what this host can do with the configured backend, without a call.
///
/// Cheap and offline: both non-ready states are local facts. A token that has
/// since expired still reads as [`Readiness::Ready`] here and surfaces as a 401
/// on the call that used it — which is the right place for it, because a flaky
/// network must never read as "sign in again".
pub fn readiness(env: &HashMap<String, String>, backend: &BackendConfig) -> Readiness {
    if backend.base_url.trim().is_empty() {
        return Readiness::Unusable("no backend base URL is configured".to_string());
    }
    let session = crate::auth::session_token(env);
    match crate::auth::resolve_backend_token(env, backend, session.as_deref()) {
        Some(_) => Readiness::Ready,
        None => Readiness::SignedOut,
    }
}

/// Confirm the backend actually answers, for a host that wants to know before
/// it paints.
///
/// Uses the session list because it is the cheapest read that goes through the
/// same client every drive method does — a host that can list sessions can
/// submit to one. A rejected call reads as [`Readiness::SignedOut`] only when
/// the backend says the credential is bad; every other failure is transient and
/// stays [`Readiness::Ready`], so a dropped connection does not send an operator
/// to a login screen.
pub async fn probe(client: &MedullaClient) -> Readiness {
    match client.list_sessions().await {
        Ok(_) => Readiness::Ready,
        Err(err) if is_unauthorized(&err) => Readiness::SignedOut,
        Err(err) => {
            tracing::debug!("[cloud_runtime] readiness probe failed, assuming ready: {err}");
            Readiness::Ready
        }
    }
}

/// Which higher-precedence source supplies a credential, if any.
///
/// "Higher precedence" means it outranks the stored session in
/// [`crate::auth::resolve_backend_token`], so signing in again cannot displace
/// it: the fresh JWT lands in the store and the resolver keeps preferring this.
/// Returns an operator-facing description of where to go and change it.
///
/// A host whose credential comes from here and is *rejected* therefore cannot be
/// fixed by a login screen, and telling it to try one is a loop.
pub fn external_credential_source(
    env: &HashMap<String, String>,
    backend: &BackendConfig,
) -> Option<String> {
    if backend.token.is_some() {
        return Some("`backend.token` in the config".to_string());
    }
    // The same emptiness rule `resolve_backend_token` applies, which rejects
    // only `""`. Trimming here instead meant a whitespace-only variable was
    // *selected* as the bearer and *not* reported as an external source — so a
    // rejected probe opened the login flow, stored a lower-precedence JWT, and
    // rebuilt a client that picked the whitespace value again.
    let named = env
        .get(&backend.token_env)
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    named.then(|| format!("the `{}` environment variable", backend.token_env))
}

/// Whether a client error means the credential was rejected.
///
/// Matched on the status *and* the backend's `errorCode`, because the two carry
/// different halves of the answer: a 401 is unambiguous, and `TOKEN_EXPIRED`
/// arrives on some routes inside a 200-shaped `{"success": false}` envelope
/// where no status is attached at all.
fn is_unauthorized(err: &crate::client::ClientError) -> bool {
    let crate::client::ClientError::Api {
        status, error_code, ..
    } = err
    else {
        return false;
    };
    matches!(status, Some(401 | 403))
        || matches!(
            error_code.as_deref(),
            Some("TOKEN_EXPIRED" | "SESSION_EXPIRED" | "UNAUTHORIZED")
        )
}

#[cfg(test)]
mod tests;
