//! The RFC 8252 loopback OAuth flow: bind an ephemeral loopback port, classify
//! and answer the browser callback, and capture the JWT. Holds the
//! [`LoopbackListener`], the [`start_loopback`]/[`run_login_flow`] entry points,
//! the pure request classifier, and the browser opener.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::types::{LoginError, LoopbackConfig, Provider};
use super::url::{login_url, parse_target, random_state_nonce};

/// Bounded per-connection read buffer, and the per-connection read timeout —
/// mirrors the reference listener so a hung or oversized connection can't stall
/// or exhaust the accept loop.
const READ_BUFFER_BYTES: usize = 8 * 1024;
const PER_CONNECTION_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Outcome of classifying one HTTP request received by the loopback accept loop.
/// Extracted as a pure function so routing can be unit-tested without a socket.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum RequestOutcome {
    /// `GET /auth` with a matching `state=` nonce. The caller extracts the
    /// `token`/`error` params from `callback_url` and finishes.
    AuthCallback { callback_url: String },
    /// `/auth` matched but `state=` was missing or wrong. Caller sends 400 and
    /// keeps waiting.
    StateMismatch,
    /// Path is not `/auth`. Caller sends 404 and keeps waiting.
    NotFound,
    /// Method is not GET. Caller sends 405 and keeps waiting.
    MethodNotAllowed,
}

/// Parse the request target (path + query) out of an HTTP/1.x request head,
/// returning `None` for a non-GET method.
fn parse_get_target(head: &str) -> Option<&str> {
    let first_line = head.split("\r\n").next()?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    method.eq_ignore_ascii_case("GET").then_some(target)
}

/// Return the raw (un-decoded) value of a query key, if present.
fn raw_query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// Classify one HTTP/1.x request against the loopback accept loop. Pure: mirrors
/// the reference `loopback_oauth::classify_request`.
pub(super) fn classify_request(
    head: &str,
    expected_state: &str,
    bound_port: u16,
) -> RequestOutcome {
    let target = match parse_get_target(head) {
        Some(t) => t,
        None => return RequestOutcome::MethodNotAllowed,
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/auth" {
        return RequestOutcome::NotFound;
    }
    match raw_query_value(query, "state") {
        Some(s) if s == expected_state => RequestOutcome::AuthCallback {
            callback_url: format!("http://127.0.0.1:{bound_port}{target}"),
        },
        _ => RequestOutcome::StateMismatch,
    }
}

/// A bound loopback listener with its state nonce and the login URL to open.
/// Splitting "start" (immediate: port/url/state) from "await the callback" lets
/// the TUI open the browser and render the waiting screen before blocking, while
/// the CLI drives both back-to-back via [`run_login_flow`].
pub struct LoopbackListener {
    listener: TcpListener,
    port: u16,
    state: String,
    login_url: String,
}

impl LoopbackListener {
    /// The loopback port the browser callback lands on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The state nonce echoed back as `?state=` and validated on the callback.
    pub fn state(&self) -> &str {
        &self.state
    }

    /// The backend login URL to open in the browser (carries the loopback
    /// `redirectUri` with the state nonce already embedded).
    pub fn login_url(&self) -> &str {
        &self.login_url
    }

    /// Wait up to `timeout` for the browser to complete the round-trip and
    /// return the captured JWT. `/auth` requests with a missing or mismatched
    /// `state` are rejected (400) and the wait continues.
    pub async fn await_callback(
        &self,
        timeout: Duration,
    ) -> std::result::Result<String, LoginError> {
        match tokio::time::timeout(
            timeout,
            accept_token(&self.listener, &self.state, self.port),
        )
        .await
        {
            Ok(res) => res,
            Err(_) => Err(LoginError::Timeout),
        }
    }
}

/// Bind an ephemeral loopback port and build the state nonce + login URL. The
/// nonce is appended to the `redirectUri` before it reaches the backend.
pub async fn start_loopback(
    base_url: &str,
    provider: Provider,
) -> std::result::Result<LoopbackListener, LoginError> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let state = random_state_nonce();
    let login_url = login_url(base_url, provider, port, &state);
    Ok(LoopbackListener {
        listener,
        port,
        state,
        login_url,
    })
}

/// Run the loopback login flow: bind an ephemeral port, open the browser (via
/// the injected `open` closure), and wait for the backend's redirect carrying a
/// JWT. Returns the captured JWT.
pub async fn run_login_flow(
    base_url: &str,
    provider: Provider,
    cfg: LoopbackConfig,
    open: impl Fn(&str),
) -> std::result::Result<String, LoginError> {
    let lb = start_loopback(base_url, provider).await?;

    eprintln!(
        "Open this URL in your browser to log in:\n  {}",
        lb.login_url()
    );
    if !cfg.no_browser {
        open(lb.login_url());
    }

    lb.await_callback(cfg.timeout).await
}

/// Accept loopback connections until one is a valid `GET /auth` callback with a
/// matching `state` nonce carrying a `token` or `error`. Non-loopback peers are
/// dropped; non-GET → 405; non-`/auth` → 404; missing/wrong state → 400; each
/// keeps the wait alive.
async fn accept_token(
    listener: &TcpListener,
    expected_state: &str,
    bound_port: u16,
) -> std::result::Result<String, LoginError> {
    loop {
        let (mut sock, peer) = listener.accept().await?;
        if !peer.ip().is_loopback() {
            let _ = sock.shutdown().await;
            continue;
        }
        let head = match read_request_head(&mut sock).await {
            Ok(Some(h)) => h,
            _ => continue,
        };
        match classify_request(&head, expected_state, bound_port) {
            RequestOutcome::MethodNotAllowed => {
                let _ = write_response(&mut sock, "405 Method Not Allowed", NOT_FOUND_HTML).await;
            }
            RequestOutcome::NotFound => {
                let _ = write_response(&mut sock, "404 Not Found", NOT_FOUND_HTML).await;
            }
            RequestOutcome::StateMismatch => {
                let _ = write_response(&mut sock, "400 Bad Request", "state mismatch").await;
            }
            RequestOutcome::AuthCallback { callback_url } => {
                let (_, params) = parse_target(&callback_url);
                if let Some(err) = params.get("error") {
                    let _ = write_response(&mut sock, "200 OK", &error_html(err)).await;
                    return Err(LoginError::Backend(err.clone()));
                }
                if let Some(token) = params.get("token") {
                    if !token.is_empty() {
                        let _ = write_response(&mut sock, "200 OK", SUCCESS_HTML).await;
                        return Ok(token.clone());
                    }
                }
                // Valid state but neither token nor error: ignore, keep waiting.
                let _ = write_response(&mut sock, "404 Not Found", NOT_FOUND_HTML).await;
            }
        }
    }
}

/// Read one HTTP request head (bounded to [`READ_BUFFER_BYTES`], with a
/// [`PER_CONNECTION_READ_TIMEOUT`]) and return the raw head string. A read
/// timeout or transport error yields `None` so the accept loop skips it.
async fn read_request_head(sock: &mut tokio::net::TcpStream) -> io::Result<Option<String>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = match tokio::time::timeout(PER_CONNECTION_READ_TIMEOUT, sock.read(&mut tmp)).await {
            Ok(Ok(n)) => n,
            Ok(Err(err)) => return Err(err),
            Err(_) => return Ok(None),
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.contains(&b'\n') {
            break;
        }
        if buf.len() >= READ_BUFFER_BYTES {
            break;
        }
    }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

/// Write a tiny `Connection: close` HTML response.
async fn write_response(
    sock: &mut tokio::net::TcpStream,
    status: &str,
    body: &str,
) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    sock.write_all(response.as_bytes()).await?;
    sock.flush().await?;
    let _ = sock.shutdown().await;
    Ok(())
}

const SUCCESS_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Medulla</title></head><body style=\"font-family:system-ui;text-align:center;padding-top:4rem\"><h1>Logged in</h1><p>Return to your terminal. You can close this tab.</p><script>setTimeout(function(){window.close()},250)</script></body></html>";

const NOT_FOUND_HTML: &str =
    "<!doctype html><html><head><meta charset=\"utf-8\"></head><body></body></html>";

fn error_html(message: &str) -> String {
    let safe = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Medulla</title></head><body style=\"font-family:system-ui;text-align:center;padding-top:4rem\"><h1>Login failed</h1><p>{safe}</p></body></html>"
    )
}

/// Spawn the platform browser opener for `url` (best-effort; errors ignored).
pub fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", "start", "", url]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let mut cmd = std::process::Command::new("true");
    let _ = cmd.spawn();
}
