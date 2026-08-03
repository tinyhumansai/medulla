//! Unit tests for the onboarding orchestration: the env-owner chain, link
//! identity detection, and the headless and interactive register paths.

use super::*;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// An environment rooted at `dir`, so a test never touches the developer's home.
fn home_env(dir: &std::path::Path, extra: &[(&str, &str)]) -> HashMap<String, String> {
    let mut e = env(&[
        ("MEDULLA_HOME", dir.join("home").to_str().unwrap()),
        ("USER", "ada"),
        ("HOSTNAME", "box-1"),
    ]);
    for (k, v) in extra {
        e.insert(k.to_string(), v.to_string());
    }
    e
}

#[test]
fn env_owner_priority_order() {
    assert_eq!(
        env_owner(&env(&[("TINYPLACE_OPENHUMAN_OWNER", "@boss")])).as_deref(),
        Some("@boss")
    );
    // The link-native key wins over every legacy one.
    assert_eq!(
        env_owner(&env(&[
            ("MEDULLA_LINK_OWNER", "orchestrator-1"),
            ("TINYPLACE_HARNESS_DM_TO", "@dm"),
            ("TINYPLACE_OPENHUMAN_OWNER", "@boss"),
        ]))
        .as_deref(),
        Some("orchestrator-1")
    );
    // Harness DM_TO still wins over the generic owner.
    assert_eq!(
        env_owner(&env(&[
            ("TINYPLACE_HARNESS_DM_TO", "@dm"),
            ("TINYPLACE_OPENHUMAN_OWNER", "@boss"),
        ]))
        .as_deref(),
        Some("@dm")
    );
    // Legacy key is last.
    assert_eq!(
        env_owner(&env(&[("OPENHUMAN_OWNER_AGENT", "addr-1")])).as_deref(),
        Some("addr-1")
    );
    // Blank values are skipped.
    assert_eq!(
        env_owner(&env(&[
            ("MEDULLA_LINK_OWNER", "  "),
            ("TINYPLACE_OPENHUMAN_OWNER", "@boss"),
        ]))
        .as_deref(),
        Some("@boss")
    );
    assert_eq!(env_owner(&env(&[])), None);
}

#[test]
fn identity_present_reads_the_link_state_file_and_never_mints_one() {
    let dir = tempfile::tempdir().unwrap();
    let link_dir = dir.path().join("link");

    // No enrollment yet.
    assert!(!identity_present(&link_dir));
    // And asking must not have created anything — enrollment needs an invite
    // token and a hand-carried pair key, neither of which this code has.
    assert!(!link_dir.exists());

    std::fs::create_dir_all(&link_dir).unwrap();
    std::fs::write(medulla_link::keys::node_path(&link_dir), "{}").unwrap();
    assert!(identity_present(&link_dir));
}

#[tokio::test]
async fn headless_auto_registers_with_env_owner() {
    let dir = tempfile::tempdir().unwrap();
    let e = home_env(dir.path(), &[("MEDULLA_LINK_OWNER", "orchestrator-1")]);

    let reg = ensure_registered(&e, false, None)
        .await
        .unwrap()
        .expect("headless registers");
    assert!(reg.newly_registered);
    // <user>@<host>/<ip> — the ip segment is best-effort.
    assert!(
        reg.profile.name.starts_with("ada@box-1/"),
        "name: {}",
        reg.profile.name
    );
    assert_eq!(reg.profile.owner.as_deref(), Some("orchestrator-1"));
    assert!(reg.profile.registered_at.is_some());

    // Second call: a profile exists but this host has not enrolled, so it is
    // still not "registered" and onboarding runs again rather than serving with
    // no way to reach anyone.
    let again = ensure_registered(&e, false, None)
        .await
        .unwrap()
        .expect("registers again");
    assert!(again.newly_registered);
}

#[tokio::test]
async fn an_enrolled_host_with_a_profile_is_not_re_onboarded() {
    let dir = tempfile::tempdir().unwrap();
    let e = home_env(dir.path(), &[]);
    let link_dir = medulla_link::keys::link_dir(&crate::home::medulla_home(&e));
    std::fs::create_dir_all(&link_dir).unwrap();
    std::fs::write(medulla_link::keys::node_path(&link_dir), "{}").unwrap();

    ensure_registered(&e, false, None).await.unwrap().unwrap();
    let again = ensure_registered(&e, false, None)
        .await
        .unwrap()
        .expect("still registered");
    assert!(!again.newly_registered);
}

#[tokio::test]
async fn malformed_explicit_config_cannot_mutate_an_existing_profile() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("broken.toml");
    std::fs::write(&config_path, "[link\nstateDir = nope").unwrap();
    let e = home_env(
        dir.path(),
        &[(
            crate::config::CONFIG_PATH_ENV,
            config_path.to_str().unwrap(),
        )],
    );
    let profile_file = profile_path(&e);
    let profile = WorkerProfile {
        name: "existing-worker".to_string(),
        address: String::new(),
        owner: Some("orchestrator-1".to_string()),
        registered_at: Some("2026-01-01T00:00:00Z".to_string()),
    };
    profile.save(&profile_file).unwrap();

    let err = match ensure_registered_in(&e, false, None, dir.path()).await {
        Err(err) => err,
        Ok(_) => panic!("an explicit malformed config must stop onboarding"),
    };

    assert!(err.to_string().contains("explicit configuration failed"));
    assert_eq!(
        WorkerProfile::load(&profile_file)
            .expect("profile remains readable")
            .address,
        "",
        "validation must happen before onboarding writes the link identity"
    );
}

#[tokio::test]
async fn missing_explicit_config_cannot_mutate_an_existing_profile() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    let e = home_env(
        dir.path(),
        &[(crate::config::CONFIG_PATH_ENV, missing.to_str().unwrap())],
    );
    let profile_file = profile_path(&e);
    WorkerProfile {
        name: "existing-worker".to_string(),
        address: String::new(),
        owner: None,
        registered_at: None,
    }
    .save(&profile_file)
    .unwrap();

    let err = match ensure_registered_in(&e, false, None, dir.path()).await {
        Err(err) => err,
        Ok(_) => panic!("a missing explicit config must stop onboarding"),
    };

    assert!(err.to_string().contains("does not exist"));
    assert_eq!(WorkerProfile::load(&profile_file).unwrap().address, "");
}

#[tokio::test]
async fn headless_without_owner_still_registers() {
    let dir = tempfile::tempdir().unwrap();
    let e = home_env(dir.path(), &[("USER", "grace"), ("HOSTNAME", "node")]);
    let reg = ensure_registered(&e, false, None)
        .await
        .unwrap()
        .expect("registers with no owner");
    assert!(reg.newly_registered);
    assert_eq!(reg.profile.owner, None);
    assert!(reg.profile.name.starts_with("grace@node/"));
}

#[tokio::test]
async fn an_aborted_interactive_onboarding_registers_nothing() {
    // `None` from the UI means the operator pressed q / Ctrl-C. That must leave
    // no profile behind, or the next launch would treat them as registered under
    // a name they never agreed to.
    let dir = tempfile::tempdir().unwrap();
    let e = home_env(dir.path(), &[]);

    let outcome = ensure_registered(
        &e,
        false,
        Some(Box::new(|_ctx| Box::pin(async { Ok(None) }))),
    )
    .await
    .expect("aborting is not an error");

    assert!(outcome.is_none(), "an abort registers nothing");
    assert!(
        crate::onboarding::WorkerProfile::load(&crate::onboarding::profile_path(&e)).is_none(),
        "no profile may be written"
    );
}

#[tokio::test]
async fn the_interactive_name_and_owner_are_what_get_registered() {
    // The whole point of the callback is that the operator's choices win over
    // the machine-derived defaults.
    let dir = tempfile::tempdir().unwrap();
    let e = home_env(dir.path(), &[]);

    let reg = ensure_registered(
        &e,
        false,
        Some(Box::new(|_ctx| {
            Box::pin(async {
                Ok(Some((
                    "chosen-name".to_string(),
                    Some("@chosen".to_string()),
                )))
            })
        })),
    )
    .await
    .unwrap()
    .expect("registers");

    assert!(reg.newly_registered);
    assert_eq!(reg.profile.name, "chosen-name");
    assert_eq!(reg.profile.owner.as_deref(), Some("@chosen"));
}

#[tokio::test]
async fn the_interactive_context_carries_the_defaults_to_prefill() {
    // The UI cannot prefill sensibly unless it is handed the derived name and
    // the env owner, so assert they actually arrive.
    let dir = tempfile::tempdir().unwrap();
    let e = home_env(dir.path(), &[("MEDULLA_LINK_OWNER", "@from-env")]);

    let reg = ensure_registered(
        &e,
        false,
        Some(Box::new(|ctx| {
            Box::pin(async move {
                assert!(
                    ctx.default_name.starts_with("ada@box-1/"),
                    "derived default: {}",
                    ctx.default_name
                );
                assert_eq!(ctx.prefill_owner.as_deref(), Some("@from-env"));
                // Not enrolled, so there is no node name or forwarder to show —
                // and the screen must be handed empty strings rather than
                // invented ones.
                assert!(ctx.address.is_empty());
                assert!(ctx.endpoint.is_empty());
                Ok(Some((ctx.default_name.clone(), ctx.prefill_owner.clone())))
            })
        })),
    )
    .await
    .unwrap()
    .expect("registers");

    assert_eq!(reg.profile.owner.as_deref(), Some("@from-env"));
}
