//! Readiness and client construction over an injected environment. Both are
//! pure over their inputs, so every precedence rule is covered without a
//! backend.

use std::collections::HashMap;

use super::{client_from_config, is_unauthorized, readiness, Readiness};
use crate::client::ClientError;
use crate::config::BackendConfig;

fn backend(base_url: &str) -> BackendConfig {
    BackendConfig {
        base_url: base_url.to_string(),
        ..BackendConfig::default()
    }
}

fn env_with(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// A host with no configured deployment cannot be fixed by signing in, so it
/// must not be sent to a login screen.
#[test]
fn no_base_url_is_unusable_not_signed_out() {
    let env = env_with(&[("MEDULLA_TOKEN", "tok")]);
    assert!(matches!(
        readiness(&env, &backend("   ")),
        Readiness::Unusable(_)
    ));
    assert!(client_from_config(&env, &backend("")).is_none());
}

#[test]
fn a_configured_backend_with_no_token_is_signed_out() {
    let env = env_with(&[("MEDULLA_HOME", "/nonexistent-medulla-root")]);
    assert_eq!(
        readiness(&env, &backend("https://api.example")),
        Readiness::SignedOut
    );
}

#[test]
fn an_env_token_makes_the_host_ready() {
    let env = env_with(&[
        ("MEDULLA_TOKEN", "from-env"),
        ("MEDULLA_HOME", "/nonexistent-medulla-root"),
    ]);
    assert_eq!(
        readiness(&env, &backend("https://api.example")),
        Readiness::Ready
    );
    let client = client_from_config(&env, &backend("https://api.example")).unwrap();
    assert_eq!(client.jwt(), "from-env");
}

/// An inline `backend.token` outranks the environment, matching
/// [`crate::auth::resolve_backend_token`]'s documented precedence.
#[test]
fn an_inline_token_outranks_the_environment() {
    let env = env_with(&[("MEDULLA_TOKEN", "from-env")]);
    let mut cfg = backend("https://api.example");
    cfg.token = Some("inline".to_string());
    let client = client_from_config(&env, &cfg).unwrap();
    assert_eq!(client.jwt(), "inline");
}

/// A trailing slash must not survive into the client's base URL, or every route
/// it builds carries a double slash.
#[test]
fn the_base_url_is_trimmed() {
    let env = env_with(&[("MEDULLA_TOKEN", "tok")]);
    let client = client_from_config(&env, &backend("  https://api.example  ")).unwrap();
    assert_eq!(client.base_url(), "https://api.example");
}

#[test]
fn a_rejected_credential_is_recognized() {
    for status in [401u16, 403] {
        assert!(is_unauthorized(&ClientError::Api {
            status: Some(status),
            message: "nope".into(),
            error_code: None,
            details: None,
        }));
    }
    assert!(is_unauthorized(&ClientError::Api {
        status: None,
        message: "expired".into(),
        error_code: Some("TOKEN_EXPIRED".into()),
        details: None,
    }));
}

/// A flaky network must never read as "sign in again": every non-auth failure
/// leaves the host ready so a dropped connection does not evict the operator.
#[test]
fn a_transient_failure_is_not_a_credential_problem() {
    assert!(!is_unauthorized(&ClientError::Api {
        status: Some(503),
        message: "upstream down".into(),
        error_code: None,
        details: None,
    }));
    assert!(!is_unauthorized(&ClientError::Decode("bad json".into())));
    assert!(!is_unauthorized(&ClientError::Sse("dropped".into())));
}

/// The stored session is scoped to the deployment that issued it.
///
/// `backend.baseUrl` is layered config and the layers include
/// `<cwd>/.medulla/config.toml`, so without this a hostile checkout could aim
/// the account's bearer at a host of its choosing simply by being the directory
/// Medulla was launched in.
mod session_scope {
    use std::collections::HashMap;

    use super::super::{same_origin, session_for};
    use crate::config::BackendConfig;

    fn env_with_session(root: &std::path::Path, issuer: &str) -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert("MEDULLA_HOME".to_string(), root.display().to_string());
        env.insert("MEDULLA_USER".to_string(), "acct-1".to_string());
        let home = crate::home::medulla_home(&env);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("session.json"),
            serde_json::json!({
                "token": "production-bearer",
                "userId": "acct-1",
                "baseUrl": issuer,
            })
            .to_string(),
        )
        .unwrap();
        env
    }

    fn backend(base_url: &str) -> BackendConfig {
        BackendConfig {
            base_url: base_url.to_string(),
            ..BackendConfig::default()
        }
    }

    #[test]
    fn the_session_is_used_for_the_deployment_that_issued_it() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_with_session(dir.path(), "https://api.tinyhumans.ai");
        assert_eq!(
            session_for(&env, &backend("https://api.tinyhumans.ai")).as_deref(),
            Some("production-bearer")
        );
    }

    /// The attack this exists to stop.
    #[test]
    fn the_session_is_withheld_from_a_repository_selected_backend() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_with_session(dir.path(), "https://api.tinyhumans.ai");
        assert_eq!(
            session_for(&env, &backend("https://attacker.example")),
            None,
            "a checkout-supplied backend must not receive the account's bearer"
        );
    }

    /// The spelling differences that are not differences — the reason the
    /// original code recorded the issuer and then refused to compare it.
    #[test]
    fn spelling_variations_of_one_origin_still_match() {
        for (a, b) in [
            ("https://api.example", "https://api.example/"),
            ("https://API.Example", "https://api.example"),
            ("https://api.example:443", "https://api.example"),
            ("http://api.example:80", "http://api.example"),
            ("https://api.example/v1", "https://api.example"),
        ] {
            assert!(same_origin(a, b), "{a} and {b} are one deployment");
        }
    }

    #[test]
    fn genuinely_different_origins_do_not_match() {
        for (a, b) in [
            ("https://api.example", "https://api.evil"),
            ("https://api.example", "http://api.example"),
            ("https://api.example", "https://api.example:8443"),
            ("https://api.example", "https://sub.api.example"),
        ] {
            assert!(!same_origin(a, b), "{a} and {b} are different deployments");
        }
    }

    /// A record written before the issuer was tracked matches nothing, so it is
    /// sent nowhere — the safe direction, at the cost of one re-login.
    #[test]
    fn a_record_with_no_issuer_is_never_sent() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_with_session(dir.path(), "");
        assert_eq!(session_for(&env, &backend("https://api.example")), None);
    }
}
