//! End-to-end tests for `medulla login`: the loopback OAuth flow, the one-time
//! token redemption path, and `/auth/me` verification.
//!
//! No real browser and no real network: a fake browser closure issues the HTTP
//! GET the backend would normally trigger, and a tiny in-process stub stands in
//! for the backend's `/auth/*` endpoints.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use medulla::auth::{run_login_flow, LoopbackConfig, Provider};
use medulla::client::MedullaClient;

/// Extract the loopback port from a login URL's encoded `redirectUri`
/// (`...127.0.0.1%3A<port>%2Fauth`).
fn port_from_login_url(url: &str) -> u16 {
    let marker = "127.0.0.1%3A";
    let start = url.find(marker).expect("redirectUri present") + marker.len();
    let digits: String = url[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().expect("port digits")
}

/// Blocking GET against the loopback listener; returns the raw HTTP response.
fn blocking_get(port: u16, target: &str) -> String {
    let mut sock = TcpStream::connect(("127.0.0.1", port)).expect("connect loopback");
    let req = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    sock.write_all(req.as_bytes()).expect("write request");
    sock.flush().ok();
    let mut resp = String::new();
    sock.read_to_string(&mut resp).ok();
    resp
}

#[tokio::test]
async fn loopback_flow_captures_token_and_serves_html() {
    let response: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let resp_slot = response.clone();

    // The "browser" first pokes /favicon.ico (must be ignored), then hits the
    // real redirect URI carrying the token. It runs on a background thread so
    // the flow can proceed to accept the connections.
    let open = move |url: &str| {
        let port = port_from_login_url(url);
        let slot = resp_slot.clone();
        std::thread::spawn(move || {
            let _ignored = blocking_get(port, "/favicon.ico");
            let ok = blocking_get(port, "/auth?token=jwt-abc.def&key=auth");
            *slot.lock().unwrap() = ok;
        });
    };

    let jwt = run_login_flow(
        "http://localhost:5000",
        Provider::Google,
        LoopbackConfig::default(),
        open,
    )
    .await
    .expect("login flow succeeds");
    assert_eq!(jwt, "jwt-abc.def");

    // Give the browser thread a moment to read the success response.
    for _ in 0..50 {
        if response.lock().unwrap().contains("Logged in") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let html = response.lock().unwrap().clone();
    assert!(html.contains("200 OK"), "success status: {html}");
    assert!(html.contains("Logged in"), "success html: {html}");
}

#[tokio::test]
async fn loopback_flow_surfaces_error_param() {
    let open = move |url: &str| {
        let port = port_from_login_url(url);
        std::thread::spawn(move || {
            let _ = blocking_get(port, "/auth?error=access%20denied&key=auth");
        });
    };

    let err = run_login_flow(
        "http://localhost:5000",
        Provider::Github,
        LoopbackConfig::default(),
        open,
    )
    .await
    .expect_err("error param fails the flow");
    assert!(
        err.to_string().contains("access denied"),
        "error message: {err}"
    );
}

#[tokio::test]
async fn loopback_flow_times_out() {
    // No browser, tiny timeout → the flow gives up waiting.
    let cfg = LoopbackConfig {
        timeout: Duration::from_millis(80),
        no_browser: true,
    };
    let err = run_login_flow("http://localhost:5000", Provider::Google, cfg, |_| {})
        .await
        .expect_err("times out");
    assert!(err.to_string().contains("timed out"), "err: {err}");
}

// ---------------------------------------------------------------------------
// Backend stub for the --token and me() paths.
// ---------------------------------------------------------------------------

/// A minimal one-request-per-connection HTTP stub serving the two auth routes
/// the login command touches: `POST /auth/login-token/consume` and
/// `GET /auth/me`.
async fn start_auth_stub() -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]);
                let line = head.lines().next().unwrap_or("");
                let data = if line.contains("/auth/login-token/consume") {
                    r#"{"jwt":"jwt-from-token"}"#.to_string()
                } else if line.contains("/auth/me") {
                    r#"{"id":"user-1","email":"dev@example.com"}"#.to_string()
                } else {
                    "null".to_string()
                };
                let body = format!(r#"{{"success":true,"data":{data}}}"#);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), handle)
}

#[tokio::test]
async fn token_path_redeems_and_me_verifies() {
    let (base_url, handle) = start_auth_stub().await;

    // --token path: redeem a one-time token for a JWT.
    let client = MedullaClient::new(&base_url, String::new());
    let jwt = client
        .consume_login_token("deadbeef".repeat(8))
        .await
        .expect("redeem token");
    assert_eq!(jwt, "jwt-from-token");

    // me() verification + the describe_me summary the command prints.
    let authed = MedullaClient::new(&base_url, jwt);
    let me = authed.me().await.expect("me");
    assert_eq!(
        medulla::auth::describe_me(&me),
        "Logged in as dev@example.com (user-1)"
    );

    handle.abort();
}
