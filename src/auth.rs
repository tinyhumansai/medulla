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
/// The default location is `<config-dir>/medulla/credentials.json`; tests inject
/// an explicit path. On unix the file is written mode `0600`. A missing or
/// corrupt file is treated as "no credentials".
#[derive(Debug, Clone)]
pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    /// A store rooted at an explicit path (used by tests).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The default store under the OS config directory, if one is resolvable.
    pub fn at_default_location() -> Option<Self> {
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

/// The loopback redirect URI the backend sends the browser back to.
pub fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}/auth")
}

/// Build the backend login URL for a provider and loopback port.
pub fn login_url(base_url: &str, provider: Provider, port: u16) -> String {
    let base = base_url.trim_end_matches('/');
    format!(
        "{base}/auth/{}/login?redirect=app&redirectUri={}",
        provider.as_str(),
        percent_encode(&redirect_uri(port)),
    )
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

/// Run the loopback login flow: bind an ephemeral port, open the browser (via
/// the injected `open` closure), and wait for the backend's redirect carrying a
/// JWT. Returns the captured JWT.
pub async fn run_login_flow(
    base_url: &str,
    provider: Provider,
    cfg: LoopbackConfig,
    open: impl Fn(&str),
) -> std::result::Result<String, LoginError> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let url = login_url(base_url, provider, port);

    eprintln!("Open this URL in your browser to log in:\n  {url}");
    if !cfg.no_browser {
        open(&url);
    }

    match tokio::time::timeout(cfg.timeout, accept_token(&listener)).await {
        Ok(res) => res,
        Err(_) => Err(LoginError::Timeout),
    }
}

/// Accept loopback connections until one carries a `token` or `error` parameter
/// on `/auth`. Favicon / stray requests get a 404 and the wait continues.
async fn accept_token(listener: &TcpListener) -> std::result::Result<String, LoginError> {
    loop {
        let (mut sock, _) = listener.accept().await?;
        let target = match read_request_target(&mut sock).await {
            Ok(Some(t)) => t,
            Ok(None) => continue,
            Err(_) => continue,
        };
        let (path, params) = parse_target(&target);
        if path != "/auth" {
            let _ = write_response(&mut sock, "404 Not Found", NOT_FOUND_HTML).await;
            continue;
        }
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
        // `/auth` with neither a token nor an error: ignore and keep waiting.
        let _ = write_response(&mut sock, "404 Not Found", NOT_FOUND_HTML).await;
    }
}

/// Read one HTTP request and return its request-target (the path+query).
async fn read_request_target(sock: &mut tokio::net::TcpStream) -> io::Result<Option<String>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 2048];
    loop {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.contains(&b'\n') {
            break;
        }
        if buf.len() > 16_384 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let request_line = head.lines().next().unwrap_or("");
    Ok(request_line
        .split_whitespace()
        .nth(1)
        .map(|s| s.to_string()))
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

const SUCCESS_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Medulla</title></head><body style=\"font-family:system-ui;text-align:center;padding-top:4rem\"><h1>Logged in</h1><p>Return to your terminal.</p></body></html>";

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
        let url = login_url("http://localhost:5000/", Provider::Google, 54321);
        assert_eq!(
            url,
            "http://localhost:5000/auth/google/login?redirect=app&redirectUri=http%3A%2F%2F127.0.0.1%3A54321%2Fauth"
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
}
