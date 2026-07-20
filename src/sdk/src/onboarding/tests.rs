//! Unit tests for the onboarding orchestration: the env-owner chain, identity
//! presence detection, and the headless auto-register path.

use super::*;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn env_owner_priority_order() {
    assert_eq!(
        env_owner(&env(&[("TINYPLACE_OPENHUMAN_OWNER", "@boss")])).as_deref(),
        Some("@boss")
    );
    // Harness DM_TO wins over the generic owner.
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
            ("TINYPLACE_HARNESS_DM_TO", "  "),
            ("TINYPLACE_OPENHUMAN_OWNER", "@boss"),
        ]))
        .as_deref(),
        Some("@boss")
    );
    assert_eq!(env_owner(&env(&[])), None);
}

#[test]
fn identity_present_reads_env_and_config() {
    let dir = std::env::temp_dir().join(format!("medulla-onb-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let config_file = dir.join("config.json");

    // Nothing yet.
    assert!(!identity_present(&config_file, &env(&[])));
    // Env key present.
    assert!(identity_present(
        &config_file,
        &env(&[("TINYPLACE_SECRET_KEY", &"a".repeat(64))])
    ));
    // Blank env key is not "present".
    assert!(!identity_present(
        &config_file,
        &env(&[("TINYPLACE_SECRET_KEY", "  ")])
    ));

    // Config with a secret key.
    std::fs::write(&config_file, r#"{"secretKey":"deadbeef"}"#).unwrap();
    assert!(identity_present(&config_file, &env(&[])));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn headless_auto_registers_with_env_owner() {
    let dir = std::env::temp_dir().join(format!("medulla-onb-hl-{}-{}", std::process::id(), "a"));
    let _ = std::fs::remove_dir_all(&dir);
    let mut e = env(&[
        ("MEDULLA_HOME", dir.join("home").to_str().unwrap()),
        ("TINYPLACE_CONFIG", dir.join("tp.json").to_str().unwrap()),
        ("TINYPLACE_OPENHUMAN_OWNER", "@overseer"),
        ("USER", "ada"),
        ("HOSTNAME", "box-1"),
    ]);
    // Provide a fixed identity so no network is needed and the address is stable.
    let signer = LocalSigner::generate();
    let hex: String = signer.seed().iter().map(|b| format!("{b:02x}")).collect();
    e.insert("TINYPLACE_SECRET_KEY".to_string(), hex);

    // Not registered yet → headless path writes a profile.
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
    assert_eq!(reg.profile.owner.as_deref(), Some("@overseer"));
    assert_eq!(reg.profile.address, signer.agent_id());
    assert!(reg.profile.registered_at.is_some());

    // Second call: already registered, returns it without re-writing.
    let again = ensure_registered(&e, false, None)
        .await
        .unwrap()
        .expect("still registered");
    assert!(!again.newly_registered);
    assert_eq!(again.profile.address, signer.agent_id());

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn headless_without_owner_still_registers() {
    let dir = std::env::temp_dir().join(format!("medulla-onb-hl-{}-{}", std::process::id(), "b"));
    let _ = std::fs::remove_dir_all(&dir);
    let signer = LocalSigner::generate();
    let hex: String = signer.seed().iter().map(|b| format!("{b:02x}")).collect();
    let e = env(&[
        ("MEDULLA_HOME", dir.join("home").to_str().unwrap()),
        ("TINYPLACE_CONFIG", dir.join("tp.json").to_str().unwrap()),
        ("TINYPLACE_SECRET_KEY", &hex),
        ("USER", "grace"),
        ("HOSTNAME", "node"),
    ]);
    let reg = ensure_registered(&e, false, None)
        .await
        .unwrap()
        .expect("registers with no owner");
    assert!(reg.newly_registered);
    assert_eq!(reg.profile.owner, None);
    assert!(reg.profile.name.starts_with("grace@node/"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// A temp-dir environment with a fixed identity, so onboarding needs no network
/// and the derived address is stable across a test.
fn interactive_env(dir: &std::path::Path) -> HashMap<String, String> {
    let mut e = env(&[
        ("MEDULLA_HOME", dir.join("home").to_str().unwrap()),
        ("TINYPLACE_CONFIG", dir.join("tp.json").to_str().unwrap()),
        ("USER", "ada"),
        ("HOSTNAME", "box-1"),
    ]);
    let signer = LocalSigner::generate();
    let hex: String = signer.seed().iter().map(|b| format!("{b:02x}")).collect();
    e.insert("TINYPLACE_SECRET_KEY".to_string(), hex);
    e
}

#[tokio::test]
async fn an_aborted_interactive_onboarding_registers_nothing() {
    // `None` from the UI means the operator pressed q / Ctrl-C. That must leave
    // no profile behind, or the next launch would treat them as registered under
    // a name they never agreed to.
    let dir = tempfile::tempdir().unwrap();
    let e = interactive_env(dir.path());

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
    let e = interactive_env(dir.path());

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
    // the resolved endpoint, so assert they actually arrive.
    let dir = tempfile::tempdir().unwrap();
    let mut e = interactive_env(dir.path());
    e.insert("TINYPLACE_OPENHUMAN_OWNER".into(), "@from-env".into());

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
                assert!(!ctx.endpoint.is_empty(), "an endpoint is resolved");
                assert!(!ctx.address.is_empty(), "the identity address is passed");
                Ok(Some((ctx.default_name.clone(), ctx.prefill_owner.clone())))
            })
        })),
    )
    .await
    .unwrap()
    .expect("registers");

    assert_eq!(reg.profile.owner.as_deref(), Some("@from-env"));
}
