//! The loopback accept loop and the streaming forward to OpenRouter.
//!
//! Deliberately thin: every attribution decision lives in [`super::headers`], so
//! what remains here is transport — accept, authenticate, map a mount onto an
//! upstream path, and stream both directions without buffering.
//!
//! Streaming is the load-bearing property. A harness's main request is an SSE
//! token stream, so collecting the response before returning it would convert a
//! live stream into one long pause and break the interactive experience the TUI
//! renders. Both bodies are therefore forwarded as streams.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use futures::TryStreamExt;
use http::{HeaderMap, Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use super::types::UpstreamShape;
use super::CredentialRegistry;

/// The body type both proxied and locally-generated responses use.
type ProxyBody = BoxBody<Bytes, std::io::Error>;

/// Everything one connection needs to serve a request.
///
/// Cloned per connection rather than borrowed because each connection is spawned
/// onto its own task; all three fields are cheap to clone.
#[derive(Clone)]
struct ProxyState {
    upstream_root: String,
    registry: Arc<Mutex<CredentialRegistry>>,
    client: reqwest::Client,
}

/// Bind `127.0.0.1:0`, spawn the accept loop, and return the bound port.
///
/// Binding before returning is what lets a caller hand the port to a child
/// immediately: the listener is already accepting by the time this resolves, so
/// there is no window in which a harness could connect and be refused.
pub(super) async fn spawn(
    upstream_root: String,
    registry: Arc<Mutex<CredentialRegistry>>,
) -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let state = ProxyState {
        upstream_root,
        registry,
        // No timeout is configured: a harness turn legitimately streams for many
        // minutes, and a request timeout here would sever it mid-answer. The
        // child's own cancellation is what ends a run.
        client: reqwest::Client::builder()
            .build()
            .map_err(std::io::Error::other)?,
    };
    tokio::spawn(async move {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                // An accept error is per-connection (fd limits, a client that
                // vanished mid-handshake); tearing the listener down would take
                // every in-flight harness with it.
                Err(error) => {
                    tracing::debug!(%error, "attribution proxy accept failed");
                    continue;
                }
            };
            // Same guard as the OAuth loopback listener: nothing off this machine
            // may present a token, even if the port were somehow reachable.
            if !peer.ip().is_loopback() {
                continue;
            }
            let state = state.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let service = service_fn(move |req| handle(req, state.clone()));
                if let Err(error) = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await
                {
                    tracing::debug!(%error, "attribution proxy connection ended");
                }
            });
        }
    });
    Ok(port)
}

/// A short plain-text response for the locally-generated (non-proxied) cases.
fn text(status: StatusCode, message: &'static str) -> Response<ProxyBody> {
    let body = Full::new(Bytes::from_static(message.as_bytes()))
        .map_err(|never: Infallible| match never {})
        .boxed();
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(body)
        .expect("static response is well-formed")
}

/// Extract the caller's bearer token from either credential header.
///
/// Both spellings are accepted because the two dialects differ: an Anthropic
/// client configured with `ANTHROPIC_AUTH_TOKEN` sends `Authorization: Bearer`,
/// while one configured with an API key sends `x-api-key` bare.
fn presented_token(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    {
        let token = value.strip_prefix("Bearer ").unwrap_or(value);
        if !token.trim().is_empty() {
            return Some(token.trim().to_string());
        }
    }
    headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

/// Split a request path into its mount dialect and the remainder to forward.
///
/// `/anthropic/v1/messages` yields `(Anthropic, "/v1/messages")`; a bare
/// `/anthropic` yields an empty remainder. An unknown first segment yields
/// `None`, which the caller answers with a 404 rather than forwarding somewhere
/// unintended.
pub(super) fn split_mount(path: &str) -> Option<(UpstreamShape, &str)> {
    let trimmed = path.strip_prefix('/').unwrap_or(path);
    let (head, rest) = match trimmed.split_once('/') {
        Some((head, rest)) => (head, rest),
        None => (trimmed, ""),
    };
    let shape = UpstreamShape::ALL
        .into_iter()
        .find(|shape| shape.mount() == head)?;
    // Re-borrow from the original so the leading slash is preserved without
    // allocating; an empty remainder stays empty rather than becoming "/".
    let remainder = if rest.is_empty() {
        ""
    } else {
        &path[path.len() - rest.len() - 1..]
    };
    Some((shape, remainder))
}

/// Build the upstream URL for one request.
pub(super) fn upstream_url(
    root: &str,
    shape: UpstreamShape,
    remainder: &str,
    query: Option<&str>,
) -> String {
    let mut url = shape.upstream_base(root);
    url.push_str(remainder);
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    url
}

/// Response headers that must not be copied from the upstream response.
///
/// The body is re-framed by this hop, so upstream framing metadata would
/// describe a message that no longer exists. `content-encoding` is deliberately
/// *not* stripped: this build of `reqwest` has no decompression features, so a
/// compressed body is relayed exactly as received and the header stays true.
fn is_stripped_response_header(name: &http::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
    )
}

/// Serve one request: authenticate, re-head, forward, stream back.
async fn handle(
    req: Request<Incoming>,
    state: ProxyState,
) -> Result<Response<ProxyBody>, Infallible> {
    let Some((shape, remainder)) = split_mount(req.uri().path()) else {
        return Ok(text(StatusCode::NOT_FOUND, "unknown proxy mount"));
    };
    let Some(token) = presented_token(req.headers()) else {
        return Ok(text(StatusCode::UNAUTHORIZED, "missing proxy token"));
    };
    // Resolved before any upstream work, so an unauthenticated caller costs one
    // map lookup and never reaches OpenRouter or spends a request.
    let upstream_key = {
        let registry = state
            .registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        registry.by_token.get(&token).cloned()
    };
    let Some(upstream_key) = upstream_key else {
        return Ok(text(StatusCode::UNAUTHORIZED, "unrecognized proxy token"));
    };

    let Some(headers) = super::headers::rewrite(req.headers(), &upstream_key) else {
        return Ok(text(
            StatusCode::INTERNAL_SERVER_ERROR,
            "upstream credential is not a valid header value",
        ));
    };
    let url = upstream_url(&state.upstream_root, shape, remainder, req.uri().query());
    let method = req.method().clone();
    // Streamed, not collected: a large prompt (or an uploaded file) should not be
    // held whole in this process on its way through.
    let body = reqwest::Body::wrap_stream(req.into_body().into_data_stream());

    let upstream = match state
        .client
        .request(method, &url)
        .headers(headers)
        .body(body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "attribution proxy upstream request failed");
            return Ok(text(StatusCode::BAD_GATEWAY, "upstream request failed"));
        }
    };

    let mut builder = Response::builder().status(upstream.status());
    if let Some(out) = builder.headers_mut() {
        for (name, value) in upstream.headers() {
            if !is_stripped_response_header(name) {
                out.append(name.clone(), value.clone());
            }
        }
    }
    // `bytes_stream` yields chunks as they arrive, which is what keeps an SSE
    // token stream live end to end.
    let stream = upstream
        .bytes_stream()
        .map_ok(Frame::data)
        .map_err(std::io::Error::other);
    let body = BoxBody::new(StreamBody::new(stream));
    Ok(builder
        .body(body)
        .unwrap_or_else(|_| text(StatusCode::BAD_GATEWAY, "malformed upstream response")))
}
