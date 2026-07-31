//! Tests for the account-scoped bits of signing in that need no terminal and no
//! backend: recording which deployment a newly adopted account belongs to.

use std::collections::HashMap;

use crate::sign_in::seed_account_backend;

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

    seed_account_backend(&env, "https://staging-api.tinyhumans.ai");

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

    seed_account_backend(&env, "https://api.tinyhumans.ai");

    let body = std::fs::read_to_string(home.join("config.toml")).expect("read");
    assert!(body.contains("https://self.hosted"), "{body}");
    assert!(
        body.contains("accent"),
        "an unrelated section must survive: {body}"
    );
}

#[test]
fn a_blank_deployment_writes_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let env = env_at(tmp.path());
    medulla::home::user::write_active_user_id(tmp.path(), "acct-3").expect("adopt");

    seed_account_backend(&env, "   ");

    assert!(
        !medulla::home::medulla_home(&env)
            .join("config.toml")
            .exists(),
        "an empty base url would pin the account to nothing"
    );
}
