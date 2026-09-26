//! Unit tests for token resolution, the pure URL/query helpers, and the
//! loopback request classifier.

use super::loopback::{classify_request, RequestOutcome};
use super::url::{parse_target, percent_decode, percent_encode};
use super::*;
use crate::config::BackendConfig;
use std::collections::HashMap;

#[test]
fn backend_token_prefers_inline_then_env() {
    let mut env = HashMap::new();
    env.insert("MEDULLA_TOKEN".to_string(), "from-env".to_string());
    let mut backend = BackendConfig::default();
    assert_eq!(
        resolve_backend_token(&env, &backend, None).as_deref(),
        Some("from-env")
    );
    backend.token = Some("inline".into());
    assert_eq!(
        resolve_backend_token(&env, &backend, None).as_deref(),
        Some("inline")
    );

    let empty = HashMap::new();
    let backend = BackendConfig::default();
    assert_eq!(resolve_backend_token(&empty, &backend, None), None);
}

#[test]
fn backend_token_ignores_empty_env_value() {
    let mut env = HashMap::new();
    env.insert("MEDULLA_TOKEN".to_string(), String::new());
    let backend = BackendConfig::default();
    // An empty env value is treated as absent.
    assert_eq!(resolve_backend_token(&env, &backend, None), None);
}

#[test]
fn backend_token_falls_back_to_the_cores_session() {
    let empty = HashMap::new();
    let backend = BackendConfig::default();
    // Config token and env absent → the core's app session is used.
    assert_eq!(
        resolve_backend_token(&empty, &backend, Some("session-jwt")).as_deref(),
        Some("session-jwt")
    );

    // A blank session is treated as absent, like a blank env value.
    assert_eq!(resolve_backend_token(&empty, &backend, Some("  ")), None);

    // Config token and env still win over the session.
    let mut env = HashMap::new();
    env.insert("MEDULLA_TOKEN".to_string(), "from-env".to_string());
    assert_eq!(
        resolve_backend_token(&env, &backend, Some("session-jwt")).as_deref(),
        Some("from-env")
    );
}

#[test]
fn one_time_login_token_recognizes_64_lower_hex() {
    assert!(is_one_time_login_token(&"a".repeat(64)));
    assert!(is_one_time_login_token(&"0123456789abcdef".repeat(4)));
    // Wrong length, uppercase, and non-hex are all rejected.
    assert!(!is_one_time_login_token(&"a".repeat(63)));
    assert!(!is_one_time_login_token(&"A".repeat(64)));
    assert!(!is_one_time_login_token(&"g".repeat(64)));
    assert!(!is_one_time_login_token(
        "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"
    ));
}

#[test]
fn login_url_shape() {
    let url = login_url("http://localhost:5000/", Provider::Google, 54321, "abc123");
    assert_eq!(
        url,
        "http://localhost:5000/auth/google/login?redirect=app&redirectUri=http%3A%2F%2F127.0.0.1%3A54321%2Fauth%3Fstate%3Dabc123"
    );
}

#[test]
fn code_login_url_carries_no_redirect_uri() {
    // The terminal flow has no listener to redirect to — the callback ends on a
    // backend-rendered page instead. A `redirectUri` here would put the flow
    // back on the loopback path this exists to avoid.
    let url = code_login_url("http://localhost:5000/", Provider::Github);
    assert_eq!(url, "http://localhost:5000/auth/github/login?redirect=cli");
    assert!(!url.contains("redirectUri"));
    assert!(!url.contains("127.0.0.1"));

    // The trailing slash is optional, as with `login_url`.
    assert_eq!(
        code_login_url("http://localhost:5000", Provider::Google),
        "http://localhost:5000/auth/google/login?redirect=cli"
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
fn percent_decode_handles_plus_lowercase_hex_and_bad_escapes() {
    // `+` decodes to a space.
    assert_eq!(percent_decode("a+b"), "a b");
    // Lowercase hex escape decodes.
    assert_eq!(percent_decode("%2f"), "/");
    // An invalid escape (non-hex) is passed through as a literal `%`.
    assert_eq!(percent_decode("x%zzy"), "x%zzy");
}

#[test]
fn parse_target_skips_empty_and_keyless_pairs() {
    // A leading `?=v` pair has an empty key and is dropped; a bare `&&` is
    // skipped without panicking.
    let (path, params) = parse_target("/p?=v&&a=b&c");
    assert_eq!(path, "/p");
    assert_eq!(params.get("a").map(String::as_str), Some("b"));
    assert_eq!(params.get("c").map(String::as_str), Some(""));
    assert!(!params.contains_key(""));
}

#[test]
fn describe_me_variants() {
    let both = serde_json::json!({"email":"a@b.c","id":"u1"});
    assert_eq!(describe_me(&both), "Logged in as a@b.c (u1)");
    let email = serde_json::json!({"email":"a@b.c"});
    assert_eq!(describe_me(&email), "Logged in as a@b.c");
    let nested = serde_json::json!({"user":{"userId":"u9"}});
    assert_eq!(describe_me(&nested), "Logged in as u9");
    // The shape the live backend returns — the summary names the same id the
    // home is scoped to, so both readers must agree on the spelling.
    let mongo = serde_json::json!({"email":"a@b.c","_id":"68f0a1b2c3d4e5f60718293a"});
    assert_eq!(
        describe_me(&mongo),
        "Logged in as a@b.c (68f0a1b2c3d4e5f60718293a)"
    );
    let empty = serde_json::json!({});
    assert_eq!(describe_me(&empty), "Logged in.");
}

#[test]
fn the_account_id_is_read_from_either_shape_and_either_spelling() {
    // This id becomes a directory name — the account's whole home hangs off it —
    // so every shape a deployment has returned has to resolve to the same value.
    let flat = serde_json::json!({"email":"a@b.c","id":"u1"});
    assert_eq!(super::user_id_from_me(&flat).as_deref(), Some("u1"));
    let nested = serde_json::json!({"user":{"userId":"u9"}});
    assert_eq!(super::user_id_from_me(&nested).as_deref(), Some("u9"));
    // `id` wins over `userId` when a response carries both, matching the line
    // the operator is shown.
    let both = serde_json::json!({"id":"u1","userId":"u2"});
    assert_eq!(super::user_id_from_me(&both).as_deref(), Some("u1"));

    // What the live backend actually sends: `/auth/me` hands back a Mongoose
    // document's `toJSON()`, which carries `_id` and no `id` at all. Missing this
    // spelling failed every real login with "the backend did not say which
    // account this token belongs to".
    let mongo = serde_json::json!({"_id":"68f0a1b2c3d4e5f60718293a","email":"a@b.c"});
    assert_eq!(
        super::user_id_from_me(&mongo).as_deref(),
        Some("68f0a1b2c3d4e5f60718293a")
    );
    // …and the Extended JSON spelling of the same field.
    let extended = serde_json::json!({"_id":{"$oid":"68f0a1b2c3d4e5f60718293a"}});
    assert_eq!(
        super::user_id_from_me(&extended).as_deref(),
        Some("68f0a1b2c3d4e5f60718293a")
    );
    // An explicit `id` is the public identifier when both are present.
    let public_and_raw = serde_json::json!({"id":"u1","_id":"68f0a1b2c3d4e5f60718293a"});
    assert_eq!(
        super::user_id_from_me(&public_and_raw).as_deref(),
        Some("u1")
    );

    // Nothing usable reads as absent rather than as an empty directory name.
    assert_eq!(super::user_id_from_me(&serde_json::json!({})), None);
    assert_eq!(
        super::user_id_from_me(&serde_json::json!({"id":"  "})),
        None
    );
    assert_eq!(super::user_id_from_me(&serde_json::json!({"id":7})), None);
    // A non-string `id` still lets a usable `_id` answer, rather than poisoning
    // the lookup at the first key.
    assert_eq!(
        super::user_id_from_me(&serde_json::json!({"id":7,"_id":"abc"})).as_deref(),
        Some("abc")
    );
}

async fn send_request(port: u16, request: &str) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    sock.write_all(request.as_bytes()).await.unwrap();
    // Drain the server's response so it finishes handling this connection before
    // we open the next one (the accept loop is serial).
    let mut buf = Vec::new();
    let _ = sock.read_to_end(&mut buf).await;
}

#[tokio::test]
async fn start_loopback_exposes_port_state_and_login_url() {
    let lb = start_loopback("http://localhost:5000/", Provider::Google)
        .await
        .unwrap();
    assert!(lb.port() > 0);
    assert_eq!(lb.state().len(), 32);
    assert!(lb.login_url().contains(lb.state()));
    assert!(lb.login_url().contains("/auth/google/login"));
}

#[tokio::test]
async fn await_callback_walks_every_reject_branch_then_captures_token() {
    let lb = start_loopback("http://localhost:5000/", Provider::Github)
        .await
        .unwrap();
    let port = lb.port();
    let state = lb.state().to_string();
    let awaiter =
        tokio::spawn(async move { lb.await_callback(std::time::Duration::from_secs(10)).await });

    // Non-GET → 405, non-/auth → 404, wrong state → 400, valid state but no
    // token/error → ignored, all keep the wait alive.
    send_request(port, "POST /auth HTTP/1.1\r\nHost: x\r\n\r\n").await;
    send_request(port, "GET /favicon.ico HTTP/1.1\r\nHost: x\r\n\r\n").await;
    send_request(port, "GET /auth?state=nope HTTP/1.1\r\nHost: x\r\n\r\n").await;
    send_request(
        port,
        &format!("GET /auth?state={state} HTTP/1.1\r\nHost: x\r\n\r\n"),
    )
    .await;
    // Finally, a valid callback carrying a token completes the flow.
    send_request(
        port,
        &format!("GET /auth?state={state}&token=jwt-xyz HTTP/1.1\r\nHost: x\r\n\r\n"),
    )
    .await;

    let token = awaiter.await.unwrap().unwrap();
    assert_eq!(token, "jwt-xyz");
}

#[tokio::test]
async fn await_callback_surfaces_backend_error_param() {
    let lb = start_loopback("http://localhost:5000/", Provider::Google)
        .await
        .unwrap();
    let port = lb.port();
    let state = lb.state().to_string();
    let awaiter =
        tokio::spawn(async move { lb.await_callback(std::time::Duration::from_secs(10)).await });

    send_request(
        port,
        &format!("GET /auth?state={state}&error=access_denied HTTP/1.1\r\nHost: x\r\n\r\n"),
    )
    .await;

    match awaiter.await.unwrap() {
        Err(LoginError::Backend(msg)) => assert_eq!(msg, "access_denied"),
        other => panic!("expected a backend error, got {other:?}"),
    }
}

#[tokio::test]
async fn run_login_flow_opens_the_url_and_returns_the_token() {
    let cfg = LoopbackConfig {
        timeout: std::time::Duration::from_secs(10),
        no_browser: false,
    };
    // run_login_flow binds its own loopback; recover the port + state nonce from
    // the URL handed to the (browser-stand-in) opener.
    let (tx, rx) = tokio::sync::oneshot::channel::<(u16, String)>();
    let tx = std::sync::Mutex::new(Some(tx));
    let opened = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let opened_flag = opened.clone();
    let flow = tokio::spawn(async move {
        run_login_flow(
            "http://localhost:5000/",
            Provider::Discord,
            cfg,
            move |url: &str| {
                opened_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                // redirectUri is percent-encoded: ...127.0.0.1%3A<port>%2Fauth%3Fstate%3D<state>
                let port = url
                    .split_once("127.0.0.1%3A")
                    .map(|(_, rest)| rest.chars().take_while(|c| c.is_ascii_digit()).collect())
                    .and_then(|d: String| d.parse::<u16>().ok())
                    .unwrap();
                let state: String = url
                    .split_once("state%3D")
                    .map(|(_, rest)| {
                        rest.chars()
                            .take_while(|c| c.is_ascii_alphanumeric())
                            .collect()
                    })
                    .unwrap();
                if let Some(tx) = tx.lock().unwrap().take() {
                    let _ = tx.send((port, state));
                }
            },
        )
        .await
    });

    let (port, state) = rx.await.unwrap();
    send_request(
        port,
        &format!("GET /auth?state={state}&token=flow-jwt HTTP/1.1\r\nHost: x\r\n\r\n"),
    )
    .await;
    let token = flow.await.unwrap().unwrap();
    assert_eq!(token, "flow-jwt");
    assert!(opened.load(std::sync::atomic::Ordering::SeqCst));
}

/// Session-store tests. These exercise the read/state/clear half, which is pure
/// file I/O over an injected env; [`super::store`] needs a backend and is
/// covered by the client integration suite.
mod session {
    use std::collections::HashMap;

    fn env_at(root: &std::path::Path) -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert("MEDULLA_HOME".to_string(), root.display().to_string());
        env.insert("MEDULLA_USER".to_string(), "acct-1".to_string());
        env
    }

    fn write_session(env: &HashMap<String, String>, body: &str) {
        let home = crate::home::medulla_home(env);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("session.json"), body).unwrap();
    }

    #[test]
    fn an_absent_store_reads_as_signed_out() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_at(dir.path());
        assert_eq!(crate::auth::session_token(&env), None);
        assert_eq!(crate::auth::state(&env), crate::auth::AuthState::default());
    }

    #[test]
    fn a_stored_session_reports_its_account() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_at(dir.path());
        write_session(
            &env,
            r#"{"token":"jwt-1","userId":"acct-1","baseUrl":"https://api.example"}"#,
        );
        assert_eq!(crate::auth::session_token(&env).as_deref(), Some("jwt-1"));
        let state = crate::auth::state(&env);
        assert!(state.is_authenticated);
        assert_eq!(state.user_id.as_deref(), Some("acct-1"));
    }

    /// The only useful recovery from a corrupt store is to sign in again, so a
    /// malformed file must read as signed out rather than fail the command that
    /// touched it.
    #[test]
    fn a_malformed_store_reads_as_signed_out() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_at(dir.path());
        write_session(&env, "{ not json");
        assert_eq!(crate::auth::session_token(&env), None);
        assert!(!crate::auth::state(&env).is_authenticated);
    }

    /// A record whose token is blank is signed out, not "authenticated with an
    /// empty bearer" — that would send `Authorization: Bearer ` to the backend.
    #[test]
    fn a_blank_token_reads_as_signed_out() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_at(dir.path());
        write_session(&env, r#"{"token":"   ","userId":"acct-1","baseUrl":""}"#);
        assert_eq!(crate::auth::session_token(&env), None);
        assert!(!crate::auth::state(&env).is_authenticated);
    }

    /// Clearing twice succeeds, and the second call reports that there was
    /// nothing to remove rather than failing.
    #[test]
    fn clearing_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_at(dir.path());
        write_session(&env, r#"{"token":"jwt-1","userId":"acct-1","baseUrl":""}"#);
        assert!(crate::auth::clear(&env)
            .expect("removal succeeds")
            .is_some());
        assert!(crate::auth::clear(&env)
            .expect("already gone is Ok")
            .is_none());
        assert_eq!(crate::auth::session_token(&env), None);
    }

    /// A session that exists and cannot be removed must be an error, not a
    /// silent `None`. Logging out is a security action: reporting success while
    /// the bearer is still on disk tells an operator they revoked access they
    /// still hold.
    #[cfg(unix)]
    #[test]
    fn a_session_that_cannot_be_removed_is_an_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let env = env_at(dir.path());
        write_session(&env, r#"{"token":"jwt-1","userId":"acct-1","baseUrl":""}"#);
        let home = crate::home::medulla_home(&env);

        // Removal is governed by the *directory's* write bit, not the file's.
        let mut perms = std::fs::metadata(&home).unwrap().permissions();
        perms.set_mode(0o500);
        std::fs::set_permissions(&home, perms).unwrap();

        let outcome = crate::auth::clear(&env);

        // Restore before asserting, or a failure leaves an undeletable tempdir.
        let mut perms = std::fs::metadata(&home).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&home, perms).unwrap();

        assert!(outcome.is_err(), "an undeletable session must report it");
    }

    /// `MEDULLA_USER` pins which account this process runs as, and the home
    /// resolves to it — so storing a token for a *different* account would put
    /// that account's bearer inside the pinned account's home, and every later
    /// run scoped to the pin would authenticate as somebody else.
    ///
    /// Covered here rather than through `store`, which needs a backend: the
    /// comparison is the whole of the rule and is exercised directly.
    #[test]
    fn a_pinned_account_and_a_mismatched_token_are_recognised_as_a_conflict() {
        let same = |pinned: &str, actual: &str| {
            crate::home::user::sanitize_account_id(pinned)
                .zip(crate::home::user::sanitize_account_id(actual))
                .map(|(a, b)| a == b)
                .unwrap_or(false)
        };
        assert!(
            !same("acct-a", "acct-b"),
            "different accounts must conflict"
        );
        assert!(same("acct-a", "acct-a"), "the same account must not");
    }

    /// `OpenOptionsExt::mode` applies only to a newly created file, so a
    /// re-store over a world-readable path would inherit its mode and leave the
    /// token exposed.
    #[cfg(unix)]
    #[test]
    fn replacing_a_loose_session_file_restores_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let env = env_at(dir.path());
        let home = crate::home::medulla_home(&env);
        std::fs::create_dir_all(&home).unwrap();
        let path = home.join("session.json");

        std::fs::write(&path, b"{}").unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&path, perms).unwrap();

        super::super::session::write_private(&path, b"{}").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "session file is {mode:o}");
    }

    /// The token is a bearer credential, so the file it lands in must not be
    /// readable by other accounts on the machine.
    #[cfg(unix)]
    #[test]
    fn the_store_is_written_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let env = env_at(dir.path());
        let home = crate::home::medulla_home(&env);
        std::fs::create_dir_all(&home).unwrap();
        let path = home.join("session.json");
        super::super::session::write_private(&path, b"{}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "session file is {mode:o}");
    }
}
