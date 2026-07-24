//! Offline, deterministic coverage for identity-slot acquisition.
//!
//! Every test runs against a `tempfile` home root and an injected environment,
//! so nothing here reads the operator's real `~/.medulla` or touches the
//! network. The locks are real OS locks: `flock`/`LockFileEx` are held per open
//! file description, so two acquisitions inside this one test process contend
//! exactly as two daemons would.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ::tinyplace::{LocalSigner, Signer};

use super::{acquire_identity, acquire_identity_at, MAX_SLOTS};
use crate::tinyplace::config::{load_config, TinyplaceFileConfig};

/// An environment naming `root` as the OS home, with nothing that would pin the
/// identity — the default, pooled path.
fn pooled_env(root: &Path) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("HOME".to_string(), root.display().to_string());
    // Windows resolves the home base from USERPROFILE; set both so the same
    // test body exercises the same slot layout on either platform.
    env.insert("USERPROFILE".to_string(), root.display().to_string());
    env
}

/// The medulla home the pooled environment resolves to.
fn medulla_home(root: &Path) -> PathBuf {
    root.join(".medulla")
}

/// Hex-encode a seed the way the config file stores it.
fn seed_hex(signer: &LocalSigner) -> String {
    signer.seed().iter().map(|b| format!("{b:02x}")).collect()
}

/// Plant an existing identity at `config_path` and return its agent id.
fn plant_identity(config_path: &Path) -> String {
    let signer = LocalSigner::generate();
    let config = TinyplaceFileConfig {
        secret_key: Some(seed_hex(&signer)),
        ..Default::default()
    };
    crate::tinyplace::config::write_config(config_path, &config).expect("write planted config");
    signer.agent_id()
}

#[test]
fn slot_one_free_reuses_the_existing_identity() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = pooled_env(root.path());
    let slot_one = medulla_home(root.path())
        .join("tinyplace")
        .join("config.json");
    let planted = plant_identity(&slot_one);

    let acquired = acquire_identity(&env, root.path()).expect("acquire");

    // A machine that already had an identity keeps it: same slot, same file,
    // same address. That backward compatibility is the point of slot 1.
    assert_eq!(acquired.slot, 1);
    assert_eq!(acquired.config_path, slot_one);
    assert_eq!(acquired.identity_dir, slot_one.parent().unwrap());
    assert_eq!(acquired.signer.agent_id(), planted);
    assert!(!medulla_home(root.path()).join("workers").exists());
    // The guard names the file it holds, beside the config it protects.
    assert_eq!(
        acquired.lock.path(),
        slot_one.parent().unwrap().join("daemon.lock")
    );
    assert!(format!("{:?}", acquired.lock).contains("daemon.lock"));
}

#[test]
fn slot_one_missing_mints_it_in_place() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = pooled_env(root.path());

    let acquired = acquire_identity(&env, root.path()).expect("acquire");

    assert_eq!(acquired.slot, 1);
    // The minted key is persisted, so the next launch reuses this address.
    let written = load_config(&acquired.config_path);
    assert_eq!(
        written.secret_key.as_deref(),
        Some(seed_hex(&acquired.signer).as_str())
    );
}

#[test]
fn a_locked_slot_one_fans_out_to_workers_two() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = pooled_env(root.path());
    let planted = plant_identity(
        &medulla_home(root.path())
            .join("tinyplace")
            .join("config.json"),
    );

    let first = acquire_identity(&env, root.path()).expect("first acquire");
    let second = acquire_identity(&env, root.path()).expect("second acquire");

    assert_eq!(first.slot, 1);
    assert_eq!(second.slot, 2);
    assert_eq!(
        second.config_path,
        medulla_home(root.path())
            .join("workers")
            .join("2")
            .join("tinyplace")
            .join("config.json")
    );
    // The whole purpose: two live daemons, two different addresses.
    assert_eq!(first.signer.agent_id(), planted);
    assert_ne!(second.signer.agent_id(), first.signer.agent_id());
}

#[test]
fn a_freed_slot_is_reclaimed_with_its_key() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = pooled_env(root.path());

    let held = acquire_identity(&env, root.path()).expect("slot one");
    let second = acquire_identity(&env, root.path()).expect("slot two");
    let second_id = second.signer.agent_id();
    let second_path = second.config_path.clone();

    // A worker exits (or crashes — the OS releases the lock either way).
    drop(second);

    let reclaimed = acquire_identity(&env, root.path()).expect("reclaim");
    assert_eq!(reclaimed.slot, 2);
    assert_eq!(reclaimed.config_path, second_path);
    // A slot's key is stable once minted, so the reclaiming worker answers on
    // the address peers already know for that slot.
    assert_eq!(reclaimed.signer.agent_id(), second_id);
    drop(held);
}

#[test]
fn concurrent_acquires_never_share_a_slot() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = pooled_env(root.path());
    // Pre-create the home so the racing threads are contending for slots rather
    // than for the directory creation.
    std::fs::create_dir_all(medulla_home(root.path())).expect("home");

    let acquired: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let env = env.clone();
                let root = root.path().to_path_buf();
                scope.spawn(move || acquire_identity(&env, &root).expect("concurrent acquire"))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("thread"))
            .collect()
    });

    let mut slots: Vec<usize> = acquired.iter().map(|a| a.slot).collect();
    slots.sort_unstable();
    assert_eq!(slots, vec![1, 2, 3, 4]);
    let mut ids: Vec<String> = acquired.iter().map(|a| a.signer.agent_id()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 4, "every concurrent daemon got its own address");
}

#[test]
fn an_explicit_home_in_use_fails_loud() {
    let root = tempfile::tempdir().expect("tempdir");
    let explicit = root.path().join("pinned");
    let mut env = pooled_env(root.path());
    env.insert("MEDULLA_HOME".to_string(), explicit.display().to_string());

    let held = acquire_identity(&env, root.path()).expect("first acquire");
    assert_eq!(
        held.config_path,
        explicit.join("tinyplace").join("config.json")
    );

    let err = acquire_identity(&env, root.path()).expect_err("second acquire must fail");
    let message = err.to_string();
    assert!(message.contains("already in use"), "got: {message}");
    assert!(message.contains("config.json"), "got: {message}");
    // Fail loud means fail loud: no silent redirect to a fan-out slot.
    assert!(!explicit.join("workers").exists());
    assert!(!medulla_home(root.path()).join("workers").exists());
}

#[test]
fn an_explicit_config_path_in_use_fails_loud() {
    let root = tempfile::tempdir().expect("tempdir");
    let pinned = root.path().join("elsewhere").join("config.json");
    let mut env = pooled_env(root.path());
    env.insert("TINYPLACE_CONFIG".to_string(), pinned.display().to_string());

    let held = acquire_identity(&env, root.path()).expect("first acquire");
    assert_eq!(held.config_path, pinned);
    let err = acquire_identity(&env, root.path()).expect_err("second acquire must fail");
    assert!(err.to_string().contains("already in use"), "got: {err}");
}

#[test]
fn an_explicit_secret_key_pins_slot_one() {
    let root = tempfile::tempdir().expect("tempdir");
    let signer = LocalSigner::generate();
    let mut env = pooled_env(root.path());
    env.insert("TINYPLACE_SECRET_KEY".to_string(), seed_hex(&signer));

    let held = acquire_identity(&env, root.path()).expect("first acquire");
    assert_eq!(held.slot, 1);
    assert_eq!(held.signer.agent_id(), signer.agent_id());

    // Fanning out here would hand slot 2 the *same* key — the collision this
    // module exists to prevent — so it must refuse instead.
    let err = acquire_identity(&env, root.path()).expect_err("second acquire must fail");
    assert!(err.to_string().contains("already in use"), "got: {err}");
    assert!(!medulla_home(root.path()).join("workers").exists());
}

#[test]
fn an_empty_pin_is_not_a_pin() {
    let root = tempfile::tempdir().expect("tempdir");
    let mut env = pooled_env(root.path());
    env.insert("MEDULLA_HOME".to_string(), "   ".to_string());
    env.insert("TINYPLACE_CONFIG".to_string(), String::new());

    let first = acquire_identity(&env, root.path()).expect("first acquire");
    let second = acquire_identity(&env, root.path()).expect("second acquire");
    assert_eq!((first.slot, second.slot), (1, 2));
}

#[test]
fn a_bad_planted_key_is_reported_not_replaced() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = pooled_env(root.path());
    let slot_one = medulla_home(root.path())
        .join("tinyplace")
        .join("config.json");
    crate::tinyplace::config::write_config(
        &slot_one,
        &TinyplaceFileConfig {
            secret_key: Some("not-a-seed".to_string()),
            ..Default::default()
        },
    )
    .expect("write");

    let err = acquire_identity(&env, root.path()).expect_err("malformed key must fail");
    assert!(err.to_string().contains("64 hex chars"), "got: {err}");
}

#[test]
fn an_exhausted_pool_reports_rather_than_scanning_forever() {
    let root = tempfile::tempdir().expect("tempdir");
    let env = pooled_env(root.path());
    let held: Vec<_> = (0..MAX_SLOTS)
        .map(|_| acquire_identity(&env, root.path()).expect("fill the pool"))
        .collect();
    assert_eq!(held.last().map(|a| a.slot), Some(MAX_SLOTS));

    let err = acquire_identity(&env, root.path()).expect_err("pool is full");
    let message = err.to_string();
    assert!(
        message.contains("no free tiny.place identity slot"),
        "got: {message}"
    );
    drop(held);
}

#[test]
fn a_named_identity_is_taken_whole_or_refused_never_moved() {
    // The worker TUI resolves its identity directory from `[tinyplace]` config
    // rather than from the pool, so it names one directly. Naming it must mean
    // getting it or being told it is taken — never quietly getting another.
    let root = tempfile::tempdir().expect("tempdir");
    let env = pooled_env(root.path());
    let named = root.path().join("configured").join("config.json");
    let planted = plant_identity(&named);

    let held = acquire_identity_at(&named, &env).expect("first acquire");
    assert_eq!(held.slot, 1);
    assert_eq!(held.config_path, named);
    assert_eq!(held.signer.agent_id(), planted);

    let err = acquire_identity_at(&named, &env).expect_err("second acquire must fail");
    assert!(err.to_string().contains("already in use"), "got: {err}");
    // The pool is not consulted, so nothing is minted anywhere else.
    assert!(!medulla_home(root.path()).join("workers").exists());
}

#[test]
fn contention_is_told_apart_from_a_real_io_failure() {
    // A busy slot is routine and moves on to the next one; anything else must
    // surface, or an unwritable home would look like a full pool.
    assert!(super::is_contended(&fs2::lock_contended_error()));
    assert!(!super::is_contended(&std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "disk is read-only"
    )));
}
