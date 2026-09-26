//! Endpoint base-URL constants and the resolvers built on them.
//!
//! # The backend URL is pinned
//!
//! There is exactly one backend this binary talks to, and it is named here as a
//! constant. Nothing at run time can move it: not the environment, not the
//! config file, not a flag. [`backend_base_url`] takes no arguments precisely so
//! that it *cannot* be steered — a caller has nothing to hand it.
//!
//! This is deliberate, and it is a change from how the endpoint used to work:
//! `MEDULLA_API_URL` and `MEDULLA_STAGING` were both honoured, as was a
//! `backend.baseUrl` key in the config file. All three are gone. An operator who
//! sets them now gets production anyway, silently — there is no error to raise,
//! because there is no longer a setting to be wrong about.
//!
//! Reaching any other deployment means editing [`PROD_BACKEND_BASE_URL`] and
//! rebuilding.
//!
//! # The one hatch, and why it cannot ship
//!
//! The `test-endpoint-override` cargo feature restores `MEDULLA_API_URL`. It
//! exists because the end-to-end suites spawn the real binary against a stub
//! backend on loopback, and a binary that can only reach production cannot be
//! tested offline at all. It is off by default, so no build a user ever runs
//! has it: the hatch is a build-time capability, not a run-time setting.

/// The Medulla backend API. The only endpoint this binary will talk to.
pub const PROD_BACKEND_BASE_URL: &str = "https://api.tinyhumans.ai";

/// The backend base URL. Always [`PROD_BACKEND_BASE_URL`].
///
/// It takes no environment and no configured value because it consults neither;
/// see the module docs for why the pin is absolute.
pub fn backend_base_url() -> &'static str {
    PROD_BACKEND_BASE_URL
}

/// The test-only `MEDULLA_API_URL` hatch, compiled in only under the
/// `test-endpoint-override` feature.
///
/// Returns the trimmed value when it is set and non-empty. Absent the feature
/// this function does not exist, so nothing can call it and the pin holds; see
/// the module docs.
#[cfg(feature = "test-endpoint-override")]
pub fn endpoint_override(env: &std::collections::HashMap<String, String>) -> Option<String> {
    env.get("MEDULLA_API_URL")
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
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
/// explicitly-configured `link.forwarderUrl` > the backend base URL.
///
/// The forwarder is not a separate service on a separate host: the backend that
/// serves the API is the one that blindly forwards link datagrams. Now that the
/// backend is pinned, the derived case is always production too; the
/// `forwarderUrl` key survives because a deployment may put the forwarder
/// somewhere else, and because two endpoints on different forwarders both start
/// cleanly, report healthy, and never hear from each other — a mistake worth
/// keeping the ability to correct.
///
/// `backend_url` is the caller's backend base URL, threaded through rather than
/// read from the constant here so the two cannot drift.
pub fn resolve_forwarder_base_url(backend_url: &str, config_url: Option<&str>) -> String {
    if let Some(value) = config_url.map(str::trim).filter(|v| !v.is_empty()) {
        return value.to_string();
    }
    backend_url.to_string()
}
