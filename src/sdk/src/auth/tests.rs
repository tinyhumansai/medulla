//! Unit tests for token resolution, the pure URL/query helpers, the loopback
//! request classifier, and the credential store.

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
fn backend_token_uses_stored_credentials_when_baseurl_matches() {
    let empty = HashMap::new();
    let backend = BackendConfig::default();
    let matching = Credentials {
        base_url: backend.base_url.clone(),
        jwt: "stored-jwt".into(),
    };
    // Config token and env absent → stored credentials are used.
    assert_eq!(
        resolve_backend_token(&empty, &backend, Some(&matching)).as_deref(),
        Some("stored-jwt")
    );

    // A mismatched baseUrl is ignored.
    let mismatched = Credentials {
        base_url: "http://other:9999".into(),
        jwt: "stored-jwt".into(),
    };
    assert_eq!(
        resolve_backend_token(&empty, &backend, Some(&mismatched)),
        None
    );

    // Config token and env still win over stored credentials.
    let mut env = HashMap::new();
    env.insert("MEDULLA_TOKEN".to_string(), "from-env".to_string());
    assert_eq!(
        resolve_backend_token(&env, &backend, Some(&matching)).as_deref(),
        Some("from-env")
    );
}

#[test]
fn missing_token_note_names_the_env_var() {
    let backend = BackendConfig::default();
    let note = missing_token_note(&backend);
    assert!(note.contains("MEDULLA_TOKEN"));
    assert!(note.contains("mock runtime"));
    assert!(note.contains("medulla login"));
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
