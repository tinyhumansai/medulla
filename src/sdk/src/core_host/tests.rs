//! Unit tests for workspace/action-dir derivation.
//!
//! These cover the derivation only, not [`super::boot`] — booting the core
//! touches process globals (`OnceLock` context, singleton event bus) and cannot
//! be torn down between tests. Boot coverage belongs in an integration test
//! with one core per test binary.

use super::*;

/// `bind_workspace` and `bind_action_dir` mutate process env, which is global.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn guard() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn clear() {
    std::env::remove_var(OPENHUMAN_WORKSPACE_ENV);
    std::env::remove_var(OPENHUMAN_ACTION_DIR_ENV);
    std::env::remove_var(OPENHUMAN_BACKEND_URL_ENV);
}

#[test]
fn the_backend_api_url_is_bound_so_auth_me_hits_the_configured_deployment() {
    // The staging failure this exists for: OpenHuman resolves `/auth/me` from
    // `BACKEND_URL` and falls back to production, so a staging token verified by
    // the login flow was then handed to production to validate — and rejected.
    let _g = guard();
    clear();
    let bound = bind_backend_api_url(&HashMap::new(), "https://staging-api.tinyhumans.ai/");
    assert_eq!(bound, "https://staging-api.tinyhumans.ai");
    assert_eq!(
        std::env::var(OPENHUMAN_BACKEND_URL_ENV).unwrap(),
        "https://staging-api.tinyhumans.ai"
    );
    clear();
}

#[test]
fn an_operator_who_aimed_the_core_themselves_keeps_their_backend_url() {
    // Either spelling counts: OpenHuman's own config chain reads both, so
    // binding over `VITE_BACKEND_URL` would override an override.
    let _g = guard();
    for key in [OPENHUMAN_BACKEND_URL_ENV, OPENHUMAN_BACKEND_URL_ALT_ENV] {
        clear();
        let env = HashMap::from([(key.to_string(), "https://self.hosted".to_string())]);
        assert_eq!(
            bind_backend_api_url(&env, "https://api.tinyhumans.ai"),
            "https://self.hosted"
        );
        assert!(std::env::var(OPENHUMAN_BACKEND_URL_ENV).is_err());
    }
    clear();
}

#[test]
fn a_blank_backend_url_binds_nothing_rather_than_an_empty_endpoint() {
    let _g = guard();
    clear();
    assert_eq!(bind_backend_api_url(&HashMap::new(), "   "), "");
    assert!(std::env::var(OPENHUMAN_BACKEND_URL_ENV).is_err());
    clear();
}

#[test]
fn workspace_nests_under_the_medulla_home() {
    // Nested, not a sibling: deleting a scratch MEDULLA_HOME must take the
    // core's state with it, or the next run silently inherits stale state.
    let dir = workspace_dir(Path::new("/tmp/scratch-home"));
    assert!(dir.starts_with("/tmp/scratch-home"), "{}", dir.display());
    assert!(dir.ends_with("openhuman/workspace"), "{}", dir.display());
}

#[test]
fn bind_workspace_derives_from_medulla_home_when_unset() {
    let _g = guard();
    clear();
    let env = HashMap::new();
    let bound = bind_workspace(&env, Path::new("/tmp/scratch-home"));
    assert_eq!(bound, workspace_dir(Path::new("/tmp/scratch-home")));
    assert_eq!(
        std::env::var(OPENHUMAN_WORKSPACE_ENV).unwrap(),
        bound.to_string_lossy()
    );
    clear();
}

#[test]
fn bind_workspace_keeps_an_explicit_operator_override() {
    // Aiming the embedded core at an existing OpenHuman install is a real
    // thing to want; the derivation must not stomp it.
    let _g = guard();
    clear();
    let env = HashMap::from([(
        OPENHUMAN_WORKSPACE_ENV.to_string(),
        "/opt/openhuman/ws".to_string(),
    )]);
    let bound = bind_workspace(&env, Path::new("/tmp/scratch-home"));
    assert_eq!(bound, PathBuf::from("/opt/openhuman/ws"));
    clear();
}

#[test]
fn bind_workspace_treats_a_blank_override_as_unset() {
    // An exported-but-empty var is a common shell accident; honouring it would
    // point the core at "" and break the scratch-run isolation silently.
    let _g = guard();
    clear();
    let env = HashMap::from([(OPENHUMAN_WORKSPACE_ENV.to_string(), "   ".to_string())]);
    let bound = bind_workspace(&env, Path::new("/tmp/scratch-home"));
    assert_eq!(bound, workspace_dir(Path::new("/tmp/scratch-home")));
    clear();
}

#[test]
fn action_dir_binds_to_the_operator_root() {
    let _g = guard();
    clear();
    let env = HashMap::new();
    let bound = bind_action_dir(&env, Some(Path::new("/repos/work")));
    assert_eq!(bound, Some(PathBuf::from("/repos/work")));
    assert_eq!(
        std::env::var(OPENHUMAN_ACTION_DIR_ENV).unwrap(),
        "/repos/work"
    );
    clear();
}

#[test]
fn action_dir_left_alone_without_a_root() {
    // Better to leave OpenHuman's own default than to bind something arbitrary.
    let _g = guard();
    clear();
    let env = HashMap::new();
    assert_eq!(bind_action_dir(&env, None), None);
    assert!(std::env::var(OPENHUMAN_ACTION_DIR_ENV).is_err());
    clear();
}

#[test]
fn action_dir_keeps_an_explicit_override() {
    let _g = guard();
    clear();
    let env = HashMap::from([(
        OPENHUMAN_ACTION_DIR_ENV.to_string(),
        "/explicit".to_string(),
    )]);
    let bound = bind_action_dir(&env, Some(Path::new("/repos/work")));
    assert_eq!(bound, Some(PathBuf::from("/explicit")));
    clear();
}

// ── Medulla readiness classification ─────────────────────────────────────────

#[test]
fn a_missing_backend_url_is_unusable_not_a_login_prompt() {
    // Nothing to dial. A login screen cannot fix it, so the host must stop and
    // say so rather than asking the operator to sign in to nowhere.
    let err = CoreError::Domain {
        method: "medulla.listSessions",
        message: "no Medulla backend configured".into(),
        kind: Some("MedullaNoBaseUrl".into()),
        data: None,
        expected_user_state: true,
    };
    assert_eq!(
        classify(Err(err)),
        Readiness::Unusable("no Medulla backend configured".into())
    );
}

#[test]
fn being_signed_out_routes_to_the_login_screen() {
    let err = CoreError::Domain {
        method: "medulla.listSessions",
        message: "not signed in; no session token available".into(),
        kind: Some("MedullaNoSessionToken".into()),
        data: None,
        expected_user_state: true,
    };
    assert_eq!(classify(Err(err)), Readiness::SignedOut);
}

#[test]
fn a_compiled_out_surface_is_unusable() {
    // `Unavailable` is a build fact, not a fault, but signing in cannot conjure
    // a controller that was never registered.
    let err = CoreError::Unavailable {
        method: "medulla.listSessions",
    };
    assert!(matches!(classify(Err(err)), Readiness::Unusable(_)));
}

#[test]
fn a_transient_failure_does_not_read_as_signed_out() {
    // Sending a signed-in operator back to the login screen because one call
    // failed is worse than showing the failure.
    let err = CoreError::Rpc {
        method: "medulla.listSessions",
        message: "connection reset".into(),
    };
    assert_eq!(classify(Err(err)), Readiness::Ready);
}

#[test]
fn another_domain_rejection_does_not_read_as_signed_out() {
    // A rejection that named some other `kind` is a real error about a real
    // backend, not an absent one.
    let err = CoreError::Domain {
        method: "medulla.listSessions",
        message: "rate limited".into(),
        kind: Some("RateLimited".into()),
        data: None,
        expected_user_state: false,
    };
    assert_eq!(classify(Err(err)), Readiness::Ready);
}

#[test]
fn a_successful_call_reads_as_ready() {
    assert_eq!(classify(Ok(())), Readiness::Ready);
}
