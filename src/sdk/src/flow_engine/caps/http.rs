//! Outbound HTTP for `http_request` nodes, behind a host allowlist.
//!
//! Two guards stand between a workflow author and the network, and they are
//! deliberately separate:
//!
//! 1. **The allowlist**, which is policy: a host an operator has agreed this
//!    workflow may reach. Empty by default, so a freshly installed workflow
//!    cannot become an exfiltration path.
//! 2. **The loopback and private-range refusal**, which is not policy: reaching
//!    `127.0.0.1` or `10.x` from a workflow means reaching services that trusted
//!    the network boundary, so it is refused whatever the allowlist says.
//!
//! Credentials never appear in the graph. A node names one with an opaque
//! `connection_ref` of the form `http_cred:<name>`, resolved here against the
//! host's store and injected into the request *after* any summary of the call
//! has been taken — so a secret cannot reach a log, an approval prompt, or a
//! node's recorded output.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tinyflows::caps::HttpClient;
use tinyflows::error::{EngineError, Result};

use crate::flow_engine::settings::CapabilitySettings;

/// The `connection_ref` prefix naming an HTTP credential.
pub const HTTP_CRED_PREFIX: &str = "http_cred:";

/// A credential the host injects into an outbound request.
#[derive(Debug, Clone)]
pub struct HttpCredential {
    /// The header to set, e.g. `Authorization`.
    pub header: String,
    /// The header's value, e.g. `Bearer …`. Never logged.
    pub value: String,
}

/// The credential name inside a `connection_ref`, if it names one.
///
/// Fails closed: a `connection_ref` that is present but not an HTTP credential
/// reference is an error rather than a silently unauthenticated request, because
/// silently dropping the credential would send the call anyway.
pub fn http_cred_name(conn: Option<&str>) -> Result<Option<&str>> {
    let Some(conn) = conn.map(str::trim).filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    conn.strip_prefix(HTTP_CRED_PREFIX)
        .map(Some)
        .filter(|name| name.is_some_and(|n| !n.is_empty()))
        .ok_or_else(|| {
            EngineError::Capability(format!(
                "http_request: unrecognised connection_ref '{conn}'; expected \
                 '{HTTP_CRED_PREFIX}<name>'"
            ))
        })
}

/// Merge `cred` into `request`'s headers, returning the request to send.
///
/// Called last, after the request has been described for logs or approval, so
/// the secret exists only in the value handed to the transport.
pub fn inject_credential(mut request: Value, cred: &HttpCredential) -> Value {
    if let Some(object) = request.as_object_mut() {
        let headers = object
            .entry("headers")
            .or_insert_with(|| Value::Object(Default::default()));
        if let Some(headers) = headers.as_object_mut() {
            headers.insert(cred.header.clone(), Value::String(cred.value.clone()));
        }
    }
    request
}

/// A description of a request safe to log or show for approval: method and URL
/// only, never headers or body.
pub fn redacted_summary(request: &Value) -> String {
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .to_ascii_uppercase();
    let url = request.get("url").and_then(Value::as_str).unwrap_or("");
    format!("{method} {url}")
}

/// An [`HttpClient`] over `reqwest`, gated by the host's allowlist.
pub struct AllowlistHttpClient {
    settings: Arc<CapabilitySettings>,
    credentials: HashMap<String, HttpCredential>,
    client: reqwest::Client,
}

impl AllowlistHttpClient {
    /// A client permitting only what `settings` allows, resolving
    /// `connection_ref`s against `credentials`.
    pub fn new(
        settings: Arc<CapabilitySettings>,
        credentials: HashMap<String, HttpCredential>,
    ) -> Self {
        Self {
            settings,
            credentials,
            // Redirects are refused rather than followed. A permitted host that
            // 302s to `169.254.169.254` or to an unlisted domain would
            // otherwise walk straight past both guards, since only the first
            // URL is ever checked. A workflow that genuinely needs to follow one
            // can make the second request itself, where it is checked again.
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
        }
    }

    /// Check the URL against both guards, returning the parsed URL.
    fn permit(&self, request: &Value) -> Result<reqwest::Url> {
        let raw = request
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| EngineError::Capability("http_request: no url".to_string()))?;
        let url = reqwest::Url::parse(raw)
            .map_err(|err| EngineError::Capability(format!("http_request: invalid url: {err}")))?;

        if !matches!(url.scheme(), "http" | "https") {
            return Err(EngineError::Capability(format!(
                "http_request: refusing scheme '{}'",
                url.scheme()
            )));
        }
        let host = url
            .host_str()
            .ok_or_else(|| EngineError::Capability("http_request: url has no host".to_string()))?;
        if is_private_host(host) {
            return Err(EngineError::Capability(format!(
                "http_request: refusing '{host}': loopback and private addresses are not \
                 reachable from a workflow"
            )));
        }
        if !self.settings.http_host_allowed(host) {
            return Err(EngineError::Capability(format!(
                "http_request: '{host}' is not in the configured http allowlist"
            )));
        }
        // Last, because it is the only check that touches the network: an
        // allowlisted name must not resolve into a range the guard above
        // refuses by literal.
        let port = url
            .port_or_known_default()
            .unwrap_or(if url.scheme() == "https" { 443 } else { 80 });
        refuse_private_resolution(host, port)?;
        Ok(url)
    }
}

/// Whether an address is one a workflow must never reach.
///
/// Loopback, link-local (which includes the cloud metadata endpoint at
/// `169.254.169.254`), and the RFC 1918 ranges. Reaching any of them from a
/// workflow means reaching services that trusted the network boundary.
pub fn is_private_addr(addr: &std::net::IpAddr) -> bool {
    match addr {
        std::net::IpAddr::V4(v4) => is_private_v4(v4),
        std::net::IpAddr::V6(v6) => {
            // An IPv4-mapped address is an IPv4 address wearing a hat:
            // `::ffff:127.0.0.1` reaches loopback just as `127.0.0.1` does, so
            // it must be judged by the same rules rather than falling through
            // the v6 checks below.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_private_v4(&mapped);
            }
            v6.is_loopback()
                || v6.is_unspecified()
                // link-local fe80::/10
                || v6.segments()[0] & 0xffc0 == 0xfe80
                // unique-local fc00::/7 — the v6 answer to RFC 1918
                || v6.segments()[0] & 0xfe00 == 0xfc00
        }
    }
}

/// The IPv4 ranges a workflow must never reach.
fn is_private_v4(addr: &std::net::Ipv4Addr) -> bool {
    addr.is_loopback() || addr.is_private() || addr.is_link_local() || addr.is_unspecified()
}

/// Every address `host` resolves to, refused if any is private.
///
/// The textual check alone is not enough: an allowlisted name whose DNS answer
/// is `127.0.0.1` would otherwise pass both guards. Resolving makes the guard
/// depend on the network it is guarding, which is the trade — but the failure
/// mode of not resolving is an authored workflow reaching internal services,
/// and that is worse than a lookup.
///
/// A name that cannot be resolved at all is refused rather than allowed: the
/// request would fail anyway, and failing here says why.
fn refuse_private_resolution(host: &str, port: u16) -> Result<()> {
    use std::net::ToSocketAddrs;

    let resolved: Vec<std::net::SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|err| {
            EngineError::Capability(format!("http_request: cannot resolve '{host}': {err}"))
        })?
        .collect();

    if let Some(private) = resolved.iter().find(|addr| is_private_addr(&addr.ip())) {
        return Err(EngineError::Capability(format!(
            "http_request: refusing '{host}': it resolves to {}, which is loopback or private",
            private.ip()
        )));
    }
    Ok(())
}

/// Whether a host *names* loopback, a link-local address, or an RFC 1918 range.
///
/// The cheap textual guard, applied before any lookup. The authoritative check
/// is `refuse_private_resolution`, which catches the names this cannot.
pub fn is_private_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".internal") {
        return true;
    }
    if let Ok(addr) = host.parse::<std::net::IpAddr>() {
        return is_private_addr(&addr);
    }
    false
}

#[async_trait]
impl HttpClient for AllowlistHttpClient {
    async fn request(&self, request: Value, conn: Option<&str>) -> Result<Value> {
        let url = self.permit(&request)?;
        let summary = redacted_summary(&request);

        // Resolve the credential before building the request, so an unknown name
        // fails before anything leaves the process.
        let credential = match http_cred_name(conn)? {
            Some(name) => Some(self.credentials.get(name).cloned().ok_or_else(|| {
                EngineError::Capability(format!("http_request: unknown credential '{name}'"))
            })?),
            None => None,
        };
        let request = match &credential {
            Some(cred) => inject_credential(request, cred),
            None => request,
        };

        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("GET")
            .to_ascii_uppercase();
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|err| EngineError::Capability(format!("http_request: {err}")))?;

        let mut builder = self.client.request(method, url);
        if let Some(headers) = request.get("headers").and_then(Value::as_object) {
            for (name, value) in headers {
                if let Some(value) = value.as_str() {
                    builder = builder.header(name, value);
                }
            }
        }
        if let Some(body) = request.get("body") {
            builder = builder.json(body);
        }

        let response = builder
            .send()
            .await
            .map_err(|err| EngineError::Capability(format!("http_request: {summary}: {err}")))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|err| EngineError::Capability(format!("http_request: {summary}: {err}")))?;
        let json: Option<Value> = serde_json::from_str(&text).ok();

        Ok(serde_json::json!({
            "status": status,
            "text": text,
            "json": json,
        }))
    }
}
