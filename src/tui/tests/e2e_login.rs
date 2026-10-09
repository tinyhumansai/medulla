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

/// Extract the state nonce embedded in the login URL's encoded `redirectUri`
/// (`...%2Fauth%3Fstate%3D<nonce>`). The backend preserves this verbatim and the
/// browser must echo it back on the callback for the listener to accept it.
fn state_from_login_url(url: &str) -> String {
    let marker = "state%3D";
    let start = url.find(marker).expect("state present") + marker.len();
    url[start..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect()
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
    // real redirect URI carrying the state nonce and the token. It runs on a
    // background thread so the flow can proceed to accept the connections.
    let open = move |url: &str| {
        let port = port_from_login_url(url);
        let state = state_from_login_url(url);
        let slot = resp_slot.clone();
        std::thread::spawn(move || {
            let _ignored = blocking_get(port, "/favicon.ico");
            let ok = blocking_get(
                port,
                &format!("/auth?state={state}&token=jwt-abc.def&key=auth"),
            );
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
        let state = state_from_login_url(url);
        std::thread::spawn(move || {
            let _ = blocking_get(
                port,
                &format!("/auth?state={state}&error=access%20denied&key=auth"),
            );
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
async fn loopback_flow_rejects_wrong_state_then_completes() {
    // A hostile page on the same loopback origin fakes a callback with the wrong
    // state (rejected with 400 "state mismatch"), then the real browser completes
    // with the correct state — the flow must ignore the first and finish on the
    // second.
    let wrong: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let wrong_slot = wrong.clone();

    let open = move |url: &str| {
        let port = port_from_login_url(url);
        let state = state_from_login_url(url);
        let slot = wrong_slot.clone();
        std::thread::spawn(move || {
            // Forged callback with a bogus state → 400, flow keeps waiting.
            let bad = blocking_get(port, "/auth?state=deadbeefdeadbeef&token=forged");
            *slot.lock().unwrap() = bad;
            // Real callback with the correct state → completes.
            let _ = blocking_get(
                port,
                &format!("/auth?state={state}&token=real-jwt&key=auth"),
            );
        });
    };

    let jwt = run_login_flow(
        "http://localhost:5000",
        Provider::Google,
        LoopbackConfig::default(),
        open,
    )
    .await
    .expect("flow completes after the correct state");
    assert_eq!(jwt, "real-jwt");

    let bad = wrong.lock().unwrap().clone();
    assert!(bad.contains("400"), "forged callback rejected: {bad}");
    assert!(
        bad.contains("state mismatch"),
        "forged callback body: {bad}"
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

/// The `/auth/me` body the live backend returns.
///
/// `/auth/me` hands back a Mongoose document's `toJSON()`, and with no
/// `virtuals: true` transform configured that document carries `_id` and no
/// `id`. Every stub here serves this shape by default: one that invented an
/// `id` field is why a login flow that could not read a real response passed
/// its whole test suite.
const LIVE_ME: &str = r#"{"_id":"68f0a1b2c3d4e5f60718293a","email":"dev@example.com"}"#;

/// The account id inside [`LIVE_ME`] — the directory name the home must adopt.
const LIVE_ME_ID: &str = "68f0a1b2c3d4e5f60718293a";

/// A minimal one-request-per-connection HTTP stub serving the two auth routes
/// the login command touches: `POST /auth/login-token/consume` and
/// `GET /auth/me`. `me` is the raw JSON body served for the latter.
async fn start_auth_stub(me: &'static str) -> (String, tokio::task::JoinHandle<()>) {
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
                    me.to_string()
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

/// Run the installed `medulla` binary against `base_url` with a private home.
///
/// The credential and endpoint variables are cleared rather than inherited: a
/// developer's real `MEDULLA_API_URL` would otherwise point this at production
/// and redeem a token there.
///
/// The core's own variables are cleared for the same reason, and they are the
/// ones that actually bite. `medulla login` stores the session through the
/// embedded core, which validates it against `/auth/me` on whatever
/// `BACKEND_URL` / `VITE_BACKEND_URL` names — and `core_host::bind_*` treats an
/// exported value as the operator aiming the core somewhere on purpose, so it
/// does *not* override it with the stub this test just started. A developer with
/// `BACKEND_URL=https://staging-api…` in their shell therefore had the stub's
/// token checked against staging, which rejects it, and the test failed on their
/// machine while passing in a bare CI environment. `OPENHUMAN_WORKSPACE` is
/// cleared alongside them because it outranks the derived path the same way, and
/// would put this run's session in the developer's real workspace.
async fn run_medulla(
    args: Vec<String>,
    home: std::path::PathBuf,
    base_url: String,
) -> std::process::Output {
    run_medulla_with_stdin(args, home, base_url, None).await
}

/// [`run_medulla`], optionally feeding `stdin` to the child.
///
/// The terminal login flow reads its code from stdin, so exercising it end to
/// end means writing to the process rather than only reading from it.
async fn run_medulla_with_stdin(
    args: Vec<String>,
    home: std::path::PathBuf,
    base_url: String,
    stdin: Option<String>,
) -> std::process::Output {
    tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_medulla"));
        // Never report a test run to the live telemetry projects.
        cmd.env("MEDULLA_ANALYTICS_DISABLED", "1");
        cmd.args(&args)
            .current_dir(&home)
            .env("MEDULLA_HOME", &home)
            .env("MEDULLA_API_URL", &base_url)
            .env("MEDULLA_CLAUDE_SESSIONS_DIR", home.join("claude-sessions"))
            .env("MEDULLA_CODEX_SESSIONS_DIR", home.join("codex-sessions"))
            .env_remove("MEDULLA_TOKEN")
            .env_remove("MEDULLA_USER")
            .env_remove("MEDULLA_STAGING")
            .env_remove("MEDULLA_BACKEND_URL")
            .env_remove("OPENROUTER_API_KEY")
            .env_remove("BACKEND_URL")
            .env_remove("VITE_BACKEND_URL")
            .env_remove("OPENHUMAN_MEDULLA_BASE_URL")
            .env_remove("OPENHUMAN_WORKSPACE");

        let Some(input) = stdin else {
            return cmd.output().expect("the medulla binary should run");
        };

        use std::io::Write;
        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the medulla binary should run");
        child
            .stdin
            .take()
            .expect("stdin was piped")
            .write_all(input.as_bytes())
            .expect("write the pasted code");
        child.wait_with_output().expect("binary runs to completion")
    })
    .await
    .expect("binary runs to completion")
}

/// The regression this file exists for: `medulla login` against a backend that
/// spells the account id `_id` must scope the install to that account, not
/// refuse with "the backend did not say which account this token belongs to".
///
/// Driven through the real binary and real HTTP because the bug lived exactly in
/// the seam a library-level test replaces: the stub used to invent an `id` field
/// no deployment sends, so every layer agreed with itself and none agreed with
/// the backend.
// Spawns the real binary against the loopback stub, which the pinned endpoint
// forbids: only a build carrying the `test-endpoint-override` hatch can be
// aimed at it. Run these with
// `cargo test -p medulla-tui --features test-endpoint-override --test e2e_login`.
#[cfg_attr(
    not(feature = "test-endpoint-override"),
    ignore = "needs --features test-endpoint-override to reach the stub backend"
)]
#[tokio::test]
async fn login_scopes_the_home_to_the_backends_underscore_id() {
    let (base_url, handle) = start_auth_stub(LIVE_ME).await;
    let home = tempfile::TempDir::new().unwrap();

    let out = run_medulla(
        vec![
            "login".to_string(),
            "--token".to_string(),
            "deadbeef".repeat(8),
        ],
        home.path().to_path_buf(),
        base_url,
    )
    .await;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "login should succeed.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !stderr.contains("did not say which account"),
        "the id-less refusal must not fire for a real response: {stderr}"
    );
    // The id reached the greeting…
    assert!(
        stdout.contains(LIVE_ME_ID),
        "the greeting names the account: {stdout}"
    );
    // …the marker every later launch reads…
    let marker = std::fs::read_to_string(home.path().join("active_user.toml"))
        .expect("the active-user marker was written");
    assert!(
        marker.contains(LIVE_ME_ID),
        "the marker names the account: {marker}"
    );
    // …and the account's own directory, which is the whole point of the id.
    assert!(
        home.path().join(LIVE_ME_ID).is_dir(),
        "the account home was created under the id"
    );

    handle.abort();
}

/// The guard itself still holds: a response that genuinely carries no id has
/// nowhere correct to go, and must refuse before a session is stored.
// Spawns the real binary against the loopback stub, which the pinned endpoint
// forbids: only a build carrying the `test-endpoint-override` hatch can be
// aimed at it. Run these with
// `cargo test -p medulla-tui --features test-endpoint-override --test e2e_login`.
#[cfg_attr(
    not(feature = "test-endpoint-override"),
    ignore = "needs --features test-endpoint-override to reach the stub backend"
)]
#[tokio::test]
async fn login_still_refuses_a_response_with_no_account_id() {
    let (base_url, handle) = start_auth_stub(r#"{"email":"dev@example.com"}"#).await;
    let home = tempfile::TempDir::new().unwrap();

    let out = run_medulla(
        vec![
            "login".to_string(),
            "--token".to_string(),
            "deadbeef".repeat(8),
        ],
        home.path().to_path_buf(),
        base_url,
    )
    .await;

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "an id-less login must fail");
    assert!(
        stderr.contains("did not say which account"),
        "the refusal explains itself: {stderr}"
    );
    assert!(
        !home.path().join("active_user.toml").exists(),
        "nothing was adopted"
    );

    handle.abort();
}

/// `medulla login --code` is the flow for a shell with no browser of its own,
/// so the two things it must do are print a URL the operator can open elsewhere
/// and accept the code that page produces on stdin.
///
/// Driven through the real binary because the value of this path is precisely
/// that it binds nothing: a library-level test could not tell the difference
/// between "no listener needed" and "listener quietly failed".
// Spawns the real binary against the loopback stub, which the pinned endpoint
// forbids: only a build carrying the `test-endpoint-override` hatch can be
// aimed at it. Run these with
// `cargo test -p medulla-tui --features test-endpoint-override --test e2e_login`.
#[cfg_attr(
    not(feature = "test-endpoint-override"),
    ignore = "needs --features test-endpoint-override to reach the stub backend"
)]
#[tokio::test]
async fn code_path_prints_a_url_and_redeems_what_is_pasted_back() {
    let (base_url, handle) = start_auth_stub(LIVE_ME).await;
    let home = tempfile::TempDir::new().unwrap();

    let out = run_medulla_with_stdin(
        vec![
            "login".to_string(),
            "--code".to_string(),
            "--provider".to_string(),
            "github".to_string(),
        ],
        home.path().to_path_buf(),
        base_url.clone(),
        // Pasted with the trailing newline a copy from the browser carries.
        Some(format!("{}\n", "deadbeef".repeat(8))),
    )
    .await;

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("{base_url}/auth/github/login?redirect=cli")),
        "the URL to open elsewhere is printed: {stdout}"
    );
    // No listener is bound and none is advertised: a `redirectUri` here would
    // put the flow back on the loopback path this exists to avoid. (The stub
    // backend is itself on 127.0.0.1, so the address alone proves nothing.)
    assert!(
        !stdout.contains("redirectUri"),
        "no loopback redirect is requested: {stdout}"
    );
    assert!(
        out.status.success(),
        "the pasted code signs in. stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("Logged in as dev@example.com"),
        "the redeemed session is verified and greeted: {stdout}"
    );

    handle.abort();
}

/// An empty paste (or a closed stdin) fails with a sentence about what was
/// missing, rather than sending a blank code to the backend.
#[tokio::test]
async fn code_path_refuses_an_empty_paste() {
    let (base_url, handle) = start_auth_stub(LIVE_ME).await;
    let home = tempfile::TempDir::new().unwrap();

    let out = run_medulla_with_stdin(
        vec!["login".to_string(), "--code".to_string()],
        home.path().to_path_buf(),
        base_url,
        Some("   \n".to_string()),
    )
    .await;

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "an empty paste is not a login");
    assert!(stderr.contains("no code was pasted"), "why: {stderr}");

    handle.abort();
}

#[tokio::test]
async fn token_path_redeems_and_me_verifies() {
    let (base_url, handle) = start_auth_stub(LIVE_ME).await;

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
        format!("Logged in as dev@example.com ({LIVE_ME_ID})")
    );

    handle.abort();
}
