//! Tests for the account-scoped bits of signing in that need no terminal and no
//! backend: recording which deployment a newly adopted account belongs to.

use std::collections::HashMap;

use crate::sign_in::{disposition, seed_account_backend, Disposition};

fn env_at(root: &std::path::Path) -> HashMap<String, String> {
    HashMap::from([(
        "MEDULLA_HOME".to_string(),
        root.to_string_lossy().into_owned(),
    )])
}

/// The account home the seed targets — passed explicitly, because seeding runs
/// *before* the marker selects that account.
fn home_of(root: &std::path::Path, id: &str) -> std::path::PathBuf {
    root.join(id)
}

#[test]
fn a_new_account_records_the_deployment_it_signed_in_to() {
    // Without this the account home is empty, the layered load falls back to the
    // production default, and the core is bound to a deployment that never
    // issued the session just stored.
    let tmp = tempfile::tempdir().expect("tempdir");
    let env = env_at(tmp.path());
    medulla::home::user::write_active_user_id(tmp.path(), "acct-1").expect("adopt");

    seed_account_backend(
        &home_of(tmp.path(), "acct-1"),
        "https://staging-api.tinyhumans.ai",
    )
    .expect("seeds");

    let cwd = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&cwd).expect("cwd");
    let loaded = medulla::config::load_config(None, &env, &cwd).expect("load");
    assert_eq!(
        loaded.config.backend.base_url, "https://staging-api.tinyhumans.ai",
        "the account's own config names the deployment it belongs to"
    );
}

#[test]
fn an_existing_accounts_config_is_never_overwritten() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let env = env_at(tmp.path());
    medulla::home::user::write_active_user_id(tmp.path(), "acct-2").expect("adopt");
    let home = medulla::home::medulla_home(&env);
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(
        home.join("config.toml"),
        "[backend]\nbaseUrl = \"https://self.hosted\"\n\n[theme]\naccent = \"green\"\n",
    )
    .expect("write");

    let notice = seed_account_backend(&home, "https://api.tinyhumans.ai")
        .expect("reports, does not fail")
        .expect("a differing endpoint is named");
    assert!(notice.contains("self.hosted"), "{notice}");

    let body = std::fs::read_to_string(home.join("config.toml")).expect("read");
    assert!(
        body.contains("https://self.hosted"),
        "a configured endpoint is never silently rewritten: {body}"
    );
    assert!(
        !body.contains("api.tinyhumans.ai\""),
        "and the login's endpoint is not merged in beside it: {body}"
    );
    assert!(
        body.contains("accent"),
        "an unrelated section must survive: {body}"
    );
}

#[test]
fn a_config_without_a_backend_still_gets_one() {
    // The guard is on the setting, not the file: an account that has a theme and
    // no deployment is exactly the case this exists for.
    let tmp = tempfile::tempdir().expect("tempdir");
    let env = env_at(tmp.path());
    medulla::home::user::write_active_user_id(tmp.path(), "acct-4").expect("adopt");
    let home = medulla::home::medulla_home(&env);
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(home.join("config.toml"), "[theme]\naccent = \"green\"\n").expect("write");

    seed_account_backend(&home, "https://staging-api.tinyhumans.ai").expect("seeds");

    let body = std::fs::read_to_string(home.join("config.toml")).expect("read");
    assert!(body.contains("staging-api.tinyhumans.ai"), "{body}");
    assert!(body.contains("accent"), "the theme must survive: {body}");
}

#[test]
fn a_blank_deployment_writes_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let env = env_at(tmp.path());
    medulla::home::user::write_active_user_id(tmp.path(), "acct-3").expect("adopt");

    seed_account_backend(&home_of(tmp.path(), "acct-3"), "   ").expect("nothing to record");

    assert!(
        !medulla::home::medulla_home(&env)
            .join("config.toml")
            .exists(),
        "an empty base url would pin the account to nothing"
    );
}

#[test]
fn a_token_for_this_account_is_stored() {
    assert_eq!(
        disposition("acct-a", false, Some("acct-a")),
        Disposition::Store
    );
    // The override changes nothing when the ids agree — it only ever restrains
    // the marker, never the store.
    assert_eq!(
        disposition("acct-a", true, Some("acct-a")),
        Disposition::Store
    );
}

#[test]
fn a_token_for_another_account_switches_rather_than_stores() {
    assert_eq!(
        disposition("acct-a", false, Some("acct-b")),
        Disposition::Switch("acct-b".to_string()),
        "storing here would put account B's bearer in account A's credential store"
    );
    // The pre-login account is not special: a real account never inherits it.
    assert_eq!(
        disposition("local", false, Some("acct-b")),
        Disposition::Switch("acct-b".to_string())
    );
}

#[test]
fn an_override_refuses_instead_of_moving_the_shared_selection() {
    // MEDULLA_USER picks an account for one process. Adopting B here would
    // change which account every *other* launch opens.
    let Disposition::Refuse(why) = disposition("acct-a", true, Some("acct-b")) else {
        panic!("an override must not switch the shared selection");
    };
    assert!(why.contains("MEDULLA_USER"), "{why}");
    assert!(why.contains("acct-b") && why.contains("acct-a"), "{why}");
}

#[test]
fn a_token_of_unknown_ownership_is_never_stored() {
    // Including on a signed-out install: `local` would accept it, but nothing
    // would ever make it an account, so every launch would sign in again and
    // every such login would share one credential store.
    for active in ["acct-a", "local"] {
        let Disposition::Refuse(why) = disposition(active, false, None) else {
            panic!("an unverifiable token must not be stored as {active}");
        };
        assert!(why.contains("did not say which account"), "{why}");
    }
}

#[test]
fn a_trailing_slash_is_not_a_different_deployment() {
    // The stored value and the configured one are the same endpoint; reporting a
    // mismatch over punctuation would train the operator to ignore the warning.
    let tmp = tempfile::tempdir().expect("tempdir");
    let env = env_at(tmp.path());
    medulla::home::user::write_active_user_id(tmp.path(), "acct-5").expect("adopt");
    let home = medulla::home::medulla_home(&env);
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(
        home.join("config.toml"),
        "[backend]\nbaseUrl = \"https://self.hosted/\"\n",
    )
    .expect("write");

    assert_eq!(
        seed_account_backend(&home, "https://self.hosted").expect("same deployment"),
        None,
        "a trailing slash is not a different endpoint"
    );

    let body = std::fs::read_to_string(home.join("config.toml")).expect("read");
    assert!(body.contains("https://self.hosted/"), "{body}");
}

#[test]
fn a_home_that_cannot_be_written_fails_the_login() {
    // Completing a login here would leave the account pointing at a deployment
    // that never issued its session — a failure the next launch reports with
    // nothing to explain it.
    let tmp = tempfile::tempdir().expect("tempdir");
    let env = env_at(tmp.path());
    medulla::home::user::write_active_user_id(tmp.path(), "acct-6").expect("adopt");
    // A file where the account directory needs to be: creating the home fails.
    std::fs::write(medulla::home::medulla_home(&env), "not a directory").expect("write");

    assert!(
        seed_account_backend(
            &medulla::home::medulla_home(&env),
            "https://staging-api.tinyhumans.ai"
        )
        .is_err(),
        "an unwritable account must not report a completed login"
    );
}

#[test]
fn a_login_that_cannot_describe_the_account_never_selects_it() {
    // Ordering: describe, then select. The other way round leaves a *failed*
    // login with the marker already moved, so every later launch resolves to the
    // home that could not be written and skips the first-account flow that
    // would fix it.
    let tmp = tempfile::tempdir().expect("tempdir");
    let env = env_at(tmp.path());
    // A file where the account directory needs to be.
    std::fs::write(tmp.path().join("acct-7"), "not a directory").expect("write");
    let me = serde_json::json!({ "id": "acct-7" });

    let err = crate::commands::adopt_account(&env, &me, "https://staging-api.tinyhumans.ai")
        .expect_err("an unwritable account must not be adopted");
    assert!(err.contains("config could not be written"), "{err}");

    assert_eq!(
        medulla::home::user::read_active_user_id(tmp.path()),
        None,
        "the selection must not have moved to an account that cannot be written"
    );
}

#[test]
fn an_id_that_could_escape_the_root_is_refused_before_it_becomes_a_path() {
    // The switch path joins this id to the root and writes a config file there.
    // `..` or a separator must never reach that join — a hostile or broken
    // /auth/me response would otherwise touch files outside any account.
    for hostile in ["..", "../../etc", "a/b", "/absolute", ".hidden"] {
        let Disposition::Refuse(why) = disposition("acct-a", false, Some(hostile)) else {
            panic!("{hostile:?} must not be joined to the root");
        };
        assert!(why.contains("cannot be a directory name"), "{why}");
    }
}

#[test]
fn a_hostile_id_writes_nothing_anywhere() {
    // End to end through adoption: no directory created, no config written, and
    // the selection untouched.
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).expect("outside");
    let root = tmp.path().join("root");
    let env = env_at(&root);
    let me = serde_json::json!({ "id": "../outside" });

    let err = crate::commands::adopt_account(&env, &me, "https://staging-api.tinyhumans.ai")
        .expect_err("a traversing id must be refused");
    assert!(err.contains("cannot be a directory name"), "{err}");

    assert!(
        !outside.join("config.toml").exists(),
        "nothing may be written outside the account root"
    );
    assert_eq!(medulla::home::user::read_active_user_id(&root), None);
}

#[test]
fn a_refused_override_writes_no_config_for_the_other_account() {
    // The mismatch is refused before anything is persisted: a login that ends in
    // an error must not leave the authenticated account's config behind in a
    // home this process was never going to use.
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut env = env_at(tmp.path());
    env.insert(
        medulla::home::user::MEDULLA_USER_ENV.to_string(),
        "acct-pinned".to_string(),
    );
    let me = serde_json::json!({ "id": "acct-other" });

    let err = crate::commands::adopt_account(&env, &me, "https://staging-api.tinyhumans.ai")
        .expect_err("a pinned process must not adopt another account");
    assert!(err.contains("MEDULLA_USER"), "{err}");

    assert!(
        !tmp.path().join("acct-other").exists(),
        "the refused account's home must not have been created"
    );
    assert_eq!(medulla::home::user::read_active_user_id(tmp.path()), None);
}
