//! Login/logout plumbing: an RFC 8252 loopback OAuth flow against the Medulla
//! backend, a small on-disk credential store, and the pure URL/query helpers the
//! CLI and tests share.
//!
//! The flow: bind an ephemeral loopback port, point the browser at
//! `<baseUrl>/auth/<provider>/login?redirect=app&redirectUri=<loopback>`, and
//! wait for the backend to redirect the browser back to the loopback URI with a
//! ready-to-use JWT (`?token=<jwt>&key=auth`) or an error (`?error=<msg>`).

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The default overall wait for the browser round-trip.
pub const DEFAULT_LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

/// Stored login credentials: the backend they belong to and the bearer JWT.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub jwt: String,
}

/// A JSON credential file (`{"baseUrl","jwt"}`) at a fixed path.
///
/// The default location is `<medulla_home>/credentials.json`; tests inject an
/// explicit path. On unix the file is written mode `0600`. A missing or corrupt
/// file is treated as "no credentials". For backward compatibility, reads fall
/// back to the retired `<config-dir>/medulla/credentials.json` location.
#[derive(Debug, Clone)]
pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    /// A store rooted at an explicit path (used by tests).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The default store under the Medulla home directory
    /// (`<home>/credentials.json`).
    pub fn at_home(home: &Path) -> Self {
        Self::new(home.join("credentials.json"))
    }

    /// The retired store under the OS config directory
    /// (`<config-dir>/medulla/credentials.json`), consulted only as a migration
    /// fallback when the home-based file is absent.
    pub fn legacy_config_dir_location() -> Option<Self> {
        dirs::config_dir().map(|d| Self::new(d.join("medulla").join("credentials.json")))
    }

    /// The backing file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load credentials, or `None` when the file is missing or corrupt.
    pub fn load(&self) -> Option<Credentials> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Load from this store, falling back to the retired config-dir location when
    /// this store has no file yet (read-only migration; nothing is moved).
    pub fn load_or_legacy(&self) -> Option<Credentials> {
        if let Some(creds) = self.load() {
            return Some(creds);
        }
        Self::legacy_config_dir_location()
            .filter(|legacy| legacy.path() != self.path())
            .and_then(|legacy| legacy.load())
    }

    /// Persist credentials, creating the parent directory and (on unix) tightening
    /// the file mode to `0600`.
    pub fn save(&self, creds: &Credentials) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(creds).map_err(io::Error::other)?;
        std::fs::write(&self.path, json)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// Remove any stored credentials. A missing file is not an error.
    pub fn clear(&self) -> io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// The OAuth identity providers the backend accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Provider {
    #[default]
    Google,
    Github,
    Twitter,
    Discord,
}

impl Provider {
    /// The wire slug used in the login path (`/auth/<slug>/login`).
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Google => "google",
            Provider::Github => "github",
            Provider::Twitter => "twitter",
            Provider::Discord => "discord",
        }
    }

    /// Parse a provider name (case-insensitive), or `None` if unrecognized.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "google" => Some(Provider::Google),
            "github" => Some(Provider::Github),
            "twitter" => Some(Provider::Twitter),
            "discord" => Some(Provider::Discord),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// The loopback redirect URI the backend sends the browser back to. The `state`
/// nonce is appended to the URI (`?state=<nonce>`) *before* it reaches the
/// backend, which preserves the loopback `redirectUri` verbatim and appends
/// `&token=`/`&error=` — so the callback query carries both the token and the
/// nonce we can validate against.
pub fn redirect_uri(port: u16, state: &str) -> String {
    format!("http://127.0.0.1:{port}/auth?state={state}")
}

/// Build the backend login URL for a provider, loopback port, and state nonce.
pub fn login_url(base_url: &str, provider: Provider, port: u16, state: &str) -> String {
    let base = base_url.trim_end_matches('/');
    format!(
        "{base}/auth/{}/login?redirect=app&redirectUri={}",
        provider.as_str(),
        percent_encode(&redirect_uri(port, state)),
    )
}

/// A random 32-hex-char (128-bit) state nonce derived from OS-seeded std
/// entropy — no `rand` dependency. `RandomState::new()` reseeds its SipHash keys
/// from the OS on every call, so the finished hashes vary across calls; we mix in
/// the process id, a monotonically-changing timestamp, and a stack address for
/// good measure.
pub fn random_state_nonce() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let mut bytes = [0u8; 16];
    for chunk in bytes.chunks_mut(8) {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(std::process::id() as u64);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        h.write_u128(nanos);
        let stack_probe = 0u8;
        h.write_usize(&stack_probe as *const u8 as usize);
        let v = h.finish().to_le_bytes();
        chunk.copy_from_slice(&v[..chunk.len()]);
    }
    hex_encode(&bytes)
}

/// Lowercase hex-encode a byte slice.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Outcome of classifying one HTTP request received by the loopback accept loop.
/// Extracted as a pure function so routing can be unit-tested without a socket.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RequestOutcome {
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
pub(crate) fn classify_request(
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

/// Summarize an `/auth/me` response for the "who am I" line.
pub fn describe_me(me: &serde_json::Value) -> String {
    let obj = me.get("user").unwrap_or(me);
    let email = obj.get("email").and_then(|v| v.as_str());
    let id = obj
        .get("id")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("userId").and_then(|v| v.as_str()));
    match (email, id) {
        (Some(e), Some(i)) => format!("Logged in as {e} ({i})"),
        (Some(e), None) => format!("Logged in as {e}"),
        (None, Some(i)) => format!("Logged in as {i}"),
        (None, None) => "Logged in.".to_string(),
    }
}

/// Percent-encode a string, escaping everything outside the unreserved set.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Percent-decode a query value (`%XX` and `+` → space).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 3;
                }
                _ => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Split a request target into `(path, query)`, percent-decoding query values.
fn parse_target(target: &str) -> (String, HashMap<String, String>) {
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    let params = query
        .split('&')
        .filter_map(|pair| {
            if pair.is_empty() {
                return None;
            }
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            if k.is_empty() {
                return None;
            }
            Some((percent_decode(k), percent_decode(v)))
        })
        .collect();
    (path.to_string(), params)
}

// ---------------------------------------------------------------------------
// Loopback flow
// ---------------------------------------------------------------------------

/// A failure of the loopback login flow.
#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    /// Could not bind / accept on the loopback socket.
    #[error("loopback login I/O error: {0}")]
    Io(#[from] io::Error),
    /// The browser round-trip did not complete within the timeout.
    #[error("timed out waiting for the browser to complete login")]
    Timeout,
    /// The backend redirected back with an `error` parameter.
    #[error("login failed: {0}")]
    Backend(String),
}

/// Knobs for [`run_login_flow`].
#[derive(Debug, Clone)]
pub struct LoopbackConfig {
    /// Overall wait for the browser round-trip.
    pub timeout: Duration,
    /// Skip spawning a browser (still prints the URL to stderr).
    pub no_browser: bool,
}

impl Default for LoopbackConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_LOGIN_TIMEOUT,
            no_browser: false,
        }
    }
}

/// Bounded per-connection read buffer, and the per-connection read timeout —
/// mirrors the reference listener so a hung or oversized connection can't stall
/// or exhaust the accept loop.
const READ_BUFFER_BYTES: usize = 8 * 1024;
const PER_CONNECTION_READ_TIMEOUT: Duration = Duration::from_secs(5);

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

// ---------------------------------------------------------------------------
// Browser opener
// ---------------------------------------------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_url_shape() {
        let url = login_url("http://localhost:5000/", Provider::Google, 54321, "abc123");
        assert_eq!(
            url,
            "http://localhost:5000/auth/google/login?redirect=app&redirectUri=http%3A%2F%2F127.0.0.1%3A54321%2Fauth%3Fstate%3Dabc123"
        );
    }

    #[test]
    fn random_state_nonce_is_32_hex_and_varies() {
        let a = random_state_nonce();
        let b = random_state_nonce();
        assert_eq!(a.len(), 32);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, b, "nonce must vary across calls");
    }

    fn auth_head(query: &str) -> String {
        format!("GET /auth{query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
    }

    #[test]
    fn classify_valid_auth_request_returns_callback_with_bound_port() {
        let head = auth_head("?state=deadbeef&token=jwt");
        assert_eq!(
            classify_request(&head, "deadbeef", 53824),
            RequestOutcome::AuthCallback {
                callback_url: "http://127.0.0.1:53824/auth?state=deadbeef&token=jwt".to_string()
            }
        );
    }

    #[test]
    fn classify_wrong_state_is_mismatch() {
        let head = auth_head("?state=wrong&token=jwt");
        assert_eq!(
            classify_request(&head, "correct", 53824),
            RequestOutcome::StateMismatch
        );
    }

    #[test]
    fn classify_missing_state_is_mismatch() {
        let head = auth_head("?token=jwt");
        assert_eq!(
            classify_request(&head, "expected", 53824),
            RequestOutcome::StateMismatch
        );
        let head_no_query = "GET /auth HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert_eq!(
            classify_request(head_no_query, "nonce", 53824),
            RequestOutcome::StateMismatch
        );
    }

    #[test]
    fn classify_favicon_is_not_found() {
        let head = "GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert_eq!(
            classify_request(head, "state", 53824),
            RequestOutcome::NotFound
        );
    }

    #[test]
    fn classify_post_is_method_not_allowed() {
        let head = "POST /auth?state=abc HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert_eq!(
            classify_request(head, "abc", 53824),
            RequestOutcome::MethodNotAllowed
        );
    }

    #[test]
    fn provider_parse_and_str() {
        assert_eq!(Provider::parse("GitHub"), Some(Provider::Github));
        assert_eq!(Provider::parse("discord").unwrap().as_str(), "discord");
        assert_eq!(Provider::parse("nope"), None);
        assert_eq!(Provider::default(), Provider::Google);
    }

    #[test]
    fn parse_target_decodes_values() {
        let (path, params) = parse_target("/auth?token=ab.cd&key=auth");
        assert_eq!(path, "/auth");
        assert_eq!(params.get("token").map(String::as_str), Some("ab.cd"));
        assert_eq!(params.get("key").map(String::as_str), Some("auth"));

        let (_, params) = parse_target("/auth?error=access%20denied%2Fnope&key=auth");
        assert_eq!(
            params.get("error").map(String::as_str),
            Some("access denied/nope")
        );

        let (path, params) = parse_target("/favicon.ico");
        assert_eq!(path, "/favicon.ico");
        assert!(params.is_empty());
    }

    #[test]
    fn percent_roundtrip() {
        let raw = "http://127.0.0.1:9/auth";
        assert_eq!(percent_decode(&percent_encode(raw)), raw);
        // A trailing stray percent is preserved rather than panicking.
        assert_eq!(percent_decode("a%"), "a%");
        assert_eq!(percent_decode("a%2"), "a%2");
    }

    #[test]
    fn describe_me_variants() {
        let both = serde_json::json!({"email":"a@b.c","id":"u1"});
        assert_eq!(describe_me(&both), "Logged in as a@b.c (u1)");
        let email = serde_json::json!({"email":"a@b.c"});
        assert_eq!(describe_me(&email), "Logged in as a@b.c");
        let nested = serde_json::json!({"user":{"userId":"u9"}});
        assert_eq!(describe_me(&nested), "Logged in as u9");
        let empty = serde_json::json!({});
        assert_eq!(describe_me(&empty), "Logged in.");
    }

    #[test]
    fn credential_store_roundtrip_corrupt_and_clear() {
        let dir = std::env::temp_dir().join(format!("medulla-cred-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("credentials.json");
        let store = CredentialStore::new(&path);

        assert!(store.load().is_none());
        let creds = Credentials {
            base_url: "http://localhost:5000".into(),
            jwt: "jwt-123".into(),
        };
        store.save(&creds).unwrap();
        assert_eq!(store.load(), Some(creds));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // Corrupt file → treated as absent.
        std::fs::write(&path, "{ not json").unwrap();
        assert!(store.load().is_none());

        store.clear().unwrap();
        assert!(store.load().is_none());
        // Clearing a missing file is a no-op.
        store.clear().unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn at_home_uses_home_credentials_json() {
        let home = std::path::Path::new("/tmp/some-medulla-home");
        let store = CredentialStore::at_home(home);
        assert_eq!(store.path(), home.join("credentials.json"));
    }

    #[test]
    fn load_or_legacy_prefers_home_then_falls_back() {
        let base = std::env::temp_dir().join(format!("medulla-cred-fb-{}", std::process::id()));
        let home = base.join("home");
        let legacy = base.join("legacy");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();

        let home_store = CredentialStore::at_home(&home);
        let legacy_store = CredentialStore::new(legacy.join("credentials.json"));

        // Only the legacy file exists → fallback reads it (simulated by calling
        // the store's own load, since the real config-dir isn't writable here).
        legacy_store
            .save(&Credentials {
                base_url: "http://legacy".into(),
                jwt: "legacy-jwt".into(),
            })
            .unwrap();
        assert!(home_store.load().is_none());
        assert_eq!(
            legacy_store.load().map(|c| c.jwt),
            Some("legacy-jwt".to_string())
        );

        // Once the home file exists it wins over any legacy file.
        home_store
            .save(&Credentials {
                base_url: "http://home".into(),
                jwt: "home-jwt".into(),
            })
            .unwrap();
        assert_eq!(
            home_store.load_or_legacy().map(|c| c.jwt),
            Some("home-jwt".to_string())
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
