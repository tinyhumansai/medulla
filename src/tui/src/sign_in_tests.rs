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

#[test]
fn a_new_account_records_the_deployment_it_signed_in_to() {
    // Without this the account home is empty, the layered load falls back to the
    // production default, and the core is bound to a deployment that never
    // issued the session just stored.
    let tmp = tempfile::tempdir().expect("tempdir");
    let env = env_at(tmp.path());
    medulla::home::user::write_active_user_id(tmp.path(), "acct-1").expect("adopt");

    seed_account_backend(&env, "https://staging-api.tinyhumans.ai").expect("seeds");

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

    seed_account_backend(&env, "https://api.tinyhumans.ai").expect("reports, does not fail");

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

    seed_account_backend(&env, "https://staging-api.tinyhumans.ai").expect("seeds");

    let body = std::fs::read_to_string(home.join("config.toml")).expect("read");
    assert!(body.contains("staging-api.tinyhumans.ai"), "{body}");
    assert!(body.contains("accent"), "the theme must survive: {body}");
}

#[test]
fn a_blank_deployment_writes_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let env = env_at(tmp.path());
    medulla::home::user::write_active_user_id(tmp.path(), "acct-3").expect("adopt");

    seed_account_backend(&env, "   ").expect("nothing to record");

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

    seed_account_backend(&env, "https://self.hosted").expect("same deployment");

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
        seed_account_backend(&env, "https://staging-api.tinyhumans.ai").is_err(),
        "an unwritable account must not report a completed login"
    );
}
