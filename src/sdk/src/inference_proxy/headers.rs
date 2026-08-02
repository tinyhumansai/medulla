//! The pure header rewrite that makes OpenRouter attribute traffic to Medulla.
//!
//! OpenRouter credits an application by reading two request headers,
//! `HTTP-Referer` and `X-Title`. Every coding harness sets its own — Claude Code
//! names itself, so does Codex, so does OpenCode — which means a run Medulla
//! orchestrated and paid for is credited to the harness instead. This module is
//! the single place that decision is reversed: whatever the child sent is
//! discarded and Medulla's own attribution is substituted.
//!
//! It is deliberately a pure function over a [`HeaderMap`]. No I/O, no clock, no
//! environment — so the strip list and the injected values are exhaustively
//! unit-testable without a socket, and the serving loop in [`super::serve`] has
//! no policy of its own.

use http::header::{HeaderMap, HeaderName, HeaderValue};

/// The `HTTP-Referer` value OpenRouter attributes traffic by.
pub const MEDULLA_REFERER: &str = "http://medulla.tinyhumans.ai/";
/// The `X-Title` value shown on OpenRouter's app leaderboard.
pub const MEDULLA_TITLE: &str = "Medulla";

/// Hop-by-hop headers (RFC 9110 §7.6.1) plus `host`.
///
/// These describe *this* connection, not the request, so forwarding them to the
/// upstream is wrong: `transfer-encoding` and `content-length` are re-derived by
/// the outbound client, and a forwarded `host` would name the loopback listener
/// rather than OpenRouter, which TLS SNI and virtual hosting would both reject.
const CONNECTION_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

/// Headers carrying the caller's credential.
///
/// The inbound value is the loopback token minted by [`super::ProxyHandle`], not
/// an upstream credential, and it must never leave this machine. It is dropped
/// unconditionally and replaced with the real OpenRouter key.
const CREDENTIAL_HEADERS: &[&str] = &["authorization", "x-api-key"];

/// Headers by which a client identifies the *application* to OpenRouter.
///
/// `referer` is listed alongside `http-referer` because a harness that sets the
/// standard header instead of OpenRouter's spelling would otherwise slip an
/// attribution claim past the strip.
const ATTRIBUTION_HEADERS: &[&str] = &["http-referer", "referer", "x-title"];

/// Name prefixes for application-identifying headers we do not enumerate.
///
/// OpenRouter's own extension namespace and the `x-app-*` convention harnesses
/// use are open-ended, so matching by prefix is what keeps a harness from
/// introducing a new attribution header we would silently forward.
const ATTRIBUTION_PREFIXES: &[&str] = &["x-openrouter-", "x-app-"];

/// Whether `name` must be dropped before the request is forwarded upstream.
///
/// Split out from [`rewrite`] so the classification is directly testable and so
/// the reason a header is dropped stays readable at the call site.
fn is_stripped(name: &HeaderName) -> bool {
    let name = name.as_str();
    CONNECTION_HEADERS.contains(&name)
        || CREDENTIAL_HEADERS.contains(&name)
        || ATTRIBUTION_HEADERS.contains(&name)
        || ATTRIBUTION_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// Build the upstream header map for one proxied request.
///
/// Everything not classified by [`is_stripped`] is forwarded verbatim — notably
/// `anthropic-version`, `anthropic-beta`, `content-type`, `accept` and
/// `user-agent`, all of which the harness needs answered on its own terms. On
/// top of that survivor set exactly three headers are set: the real upstream
/// credential and Medulla's two attribution values.
///
/// `upstream_key` is the resolved OpenRouter API key. It is the only secret this
/// function touches and it is never logged; a key that cannot be encoded as a
/// header value yields `None` rather than a request sent unauthenticated.
///
/// Note that [`HeaderName`] comparison is already case-insensitive (`http`
/// lowercases on parse), so a harness sending `X-Title` or `x-title` is stripped
/// identically.
pub fn rewrite(inbound: &HeaderMap, upstream_key: &str) -> Option<HeaderMap> {
    let mut out = HeaderMap::with_capacity(inbound.len() + 3);
    // `iter()` yields one entry per value, so multi-valued headers survive as
    // multiple appends rather than collapsing to the first.
    for (name, value) in inbound.iter() {
        if is_stripped(name) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }

    let mut authorization = HeaderValue::from_str(&format!("Bearer {upstream_key}")).ok()?;
    // The key is a credential wherever it is rendered — mark it so a debug print
    // of the map cannot spill it into a log line.
    authorization.set_sensitive(true);
    out.insert(http::header::AUTHORIZATION, authorization);
    out.insert(
        HeaderName::from_static("http-referer"),
        HeaderValue::from_static(MEDULLA_REFERER),
    );
    out.insert(
        HeaderName::from_static("x-title"),
        HeaderValue::from_static(MEDULLA_TITLE),
    );
    Some(out)
}
