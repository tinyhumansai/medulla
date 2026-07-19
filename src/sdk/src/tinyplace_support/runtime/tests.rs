//! Unit tests for the tiny.place runtime helpers: identity bootstrap, the
//! file-backed session store round-trips, and error mapping.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{load_or_create_identity, FileSessionStore};
use tinyplace::signal::crypto::generate_x25519_keypair;
use tinyplace::signal::keys::PreKeyPair;
use tinyplace::signal::store::{SessionState, SessionStore};
use tinyplace::Signer;

/// A process/time-unique temp directory, created on demand.
fn unique_temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "tinyplace-proto-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn file_session_store_round_trips_prekeys_and_sessions() {
    let dir = unique_temp_dir("store");
    let path = FileSessionStore::default_path(&dir, "@alice:agent");
    assert!(path.ends_with("signal/_alice_agent.json"));

    let identity = generate_x25519_keypair();
    let store = FileSessionStore::new(&path, identity.clone());

    // Identity key pair comes straight back.
    let got_identity = store.identity_x25519_key_pair().await.unwrap();
    assert_eq!(got_identity.public_key, identity.public_key);
    assert_eq!(got_identity.private_key, identity.private_key);

    // Store a signed pre-key (also becomes the active one) and a one-time pre-key.
    let signed = PreKeyPair {
        key_id: "spk_1".to_string(),
        key_pair: generate_x25519_keypair(),
        signature: vec![1, 2, 3, 4, 5],
    };
    store.store_signed_pre_key(signed.clone()).await.unwrap();

    let one_time = PreKeyPair {
        key_id: "pk_7".to_string(),
        key_pair: generate_x25519_keypair(),
        signature: vec![9, 8, 7],
    };
    store.store_pre_key(one_time.clone()).await.unwrap();

    // A ratchet session with skipped keys.
    let mut skipped = HashMap::new();
    skipped.insert("abc:3".to_string(), [7u8; 32]);
    let session = SessionState {
        dh_send_key_pair: generate_x25519_keypair(),
        dh_recv_public_key: Some([2u8; 32]),
        root_key: [3u8; 32],
        send_chain_key: Some([4u8; 32]),
        recv_chain_key: None,
        send_message_number: 5,
        recv_message_number: 6,
        previous_chain_length: 7,
        skipped_keys: skipped,
    };
    store.store_session("@peer", session.clone()).await.unwrap();

    // A fresh store instance reads the same file from disk (no shared cache).
    let reopened = FileSessionStore::new(&path, identity.clone());

    let active = reopened.active_signed_pre_key().await.unwrap();
    assert_eq!(active.key_id, "spk_1");
    assert_eq!(active.key_pair.public_key, signed.key_pair.public_key);
    assert_eq!(active.signature, signed.signature);

    let got_signed = reopened.signed_pre_key("spk_1").await.unwrap().unwrap();
    assert_eq!(got_signed.key_pair.private_key, signed.key_pair.private_key);

    let got_pre = reopened.pre_key("pk_7").await.unwrap().unwrap();
    assert_eq!(got_pre.key_pair.public_key, one_time.key_pair.public_key);
    assert_eq!(got_pre.signature, one_time.signature);
    assert_eq!(reopened.all_pre_keys().await.unwrap().len(), 1);

    let got_session = reopened.session("@peer").await.unwrap().unwrap();
    assert_eq!(
        got_session.dh_send_key_pair.public_key,
        session.dh_send_key_pair.public_key
    );
    assert_eq!(got_session.dh_recv_public_key, Some([2u8; 32]));
    assert_eq!(got_session.root_key, [3u8; 32]);
    assert_eq!(got_session.send_chain_key, Some([4u8; 32]));
    assert_eq!(got_session.recv_chain_key, None);
    assert_eq!(got_session.send_message_number, 5);
    assert_eq!(got_session.recv_message_number, 6);
    assert_eq!(got_session.previous_chain_length, 7);
    assert_eq!(got_session.skipped_keys.get("abc:3"), Some(&[7u8; 32]));

    // Removal persists.
    reopened.remove_pre_key("pk_7").await.unwrap();
    reopened.remove_session("@peer").await.unwrap();
    assert!(reopened.pre_key("pk_7").await.unwrap().is_none());
    assert!(reopened.session("@peer").await.unwrap().is_none());

    // The on-disk file uses the TS-compatible camelCase layout.
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"signedPreKeys\""));
    assert!(raw.contains("\"activeSignedPreKeyId\""));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn default_path_sanitizes_caps_and_defaults_empty_owner() {
    let dir = std::path::Path::new("/tmp/x");
    // Non-alnum runs collapse to underscores.
    let p = FileSessionStore::default_path(dir, "@a/b c.d");
    assert!(p.ends_with("signal/_a_b_c_d.json"), "got {p:?}");
    // An owner with no retained chars falls back to `default`.
    let p2 = FileSessionStore::default_path(dir, "/////");
    assert!(p2.ends_with("signal/_____.json"), "got {p2:?}");
    let p3 = FileSessionStore::default_path(dir, "");
    assert!(p3.ends_with("signal/default.json"), "got {p3:?}");
    // The stem is capped at 80 chars.
    let long = "a".repeat(200);
    let p4 = FileSessionStore::default_path(dir, &long);
    let stem = p4.file_stem().unwrap().to_string_lossy().to_string();
    assert_eq!(stem.len(), 80);
}

#[tokio::test]
async fn store_reads_missing_dir_as_empty_and_reports_no_active_key() {
    // A path under a directory that does not exist yet: load() sees NotFound
    // and returns the default (empty) shape rather than erroring.
    let dir = unique_temp_dir("missing");
    let path = dir.join("nope").join("signal").join("id.json");
    let store = FileSessionStore::new(&path, generate_x25519_keypair());

    assert!(store.signed_pre_key("x").await.unwrap().is_none());
    assert!(store.pre_key("x").await.unwrap().is_none());
    assert!(store.session("@peer").await.unwrap().is_none());
    assert!(store.all_pre_keys().await.unwrap().is_empty());
    // No active signed pre-key yet → an error.
    assert!(store.active_signed_pre_key().await.is_err());

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn store_surfaces_corrupted_file_as_error() {
    let dir = unique_temp_dir("corrupt");
    let path = dir.join("signal").join("id.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"{ this is not json").unwrap();

    let store = FileSessionStore::new(&path, generate_x25519_keypair());
    assert!(store.session("@peer").await.is_err());
    assert!(store.all_pre_keys().await.is_err());

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn active_signed_pre_key_missing_target_is_error() {
    // active id points at a key not in the map → InvalidArgument.
    let dir = unique_temp_dir("dangling");
    let path = dir.join("signal").join("id.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"version":1,"signedPreKeys":{},"activeSignedPreKeyId":"ghost","preKeys":{},"sessions":{}}"#,
    )
    .unwrap();
    let store = FileSessionStore::new(&path, generate_x25519_keypair());
    assert!(store.active_signed_pre_key().await.is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn store_rejects_malformed_key_material() {
    // A session with a base64 key of the wrong length fails to deserialize.
    let dir = unique_temp_dir("badkey");
    let path = dir.join("signal").join("id.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bad = r#"{
        "version":1,
        "signedPreKeys":{},
        "activeSignedPreKeyId":null,
        "preKeys":{},
        "sessions":{
            "@peer":{
                "dhSendKeyPair":{"publicKey":"AA==","privateKey":"AA=="},
                "dhRecvPublicKey":null,
                "rootKey":"AA==",
                "sendChainKey":null,
                "recvChainKey":null,
                "sendMessageNumber":0,
                "recvMessageNumber":0,
                "previousChainLength":0,
                "skippedKeys":[]
            }
        }
    }"#;
    std::fs::write(&path, bad).unwrap();
    let store = FileSessionStore::new(&path, generate_x25519_keypair());
    // de_b32 rejects the short key.
    assert!(store.session("@peer").await.is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_or_create_identity_rejects_bad_secret_key() {
    let dir = unique_temp_dir("badhex");
    let config_path = dir.join("config.json");

    let err_string =
        |env: &HashMap<String, String>| match load_or_create_identity(&config_path, env) {
            Ok(_) => panic!("expected an error"),
            Err(e) => e.to_string(),
        };

    // Too-short hex from env.
    let mut env = HashMap::new();
    env.insert("TINYPLACE_SECRET_KEY".to_string(), "abcd".to_string());
    let err = err_string(&env);
    assert!(err.contains("64 hex chars"), "got {err}");

    // Right length but not valid hex.
    let mut env2 = HashMap::new();
    env2.insert("TINYPLACE_SECRET_KEY".to_string(), "zz".repeat(32));
    let err2 = err_string(&env2);
    assert!(err2.contains("not valid hex"), "got {err2}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_or_create_identity_ignores_blank_env_and_reads_config_seed() {
    let dir = unique_temp_dir("blankenv");
    let config_path = dir.join("config.json");

    // Seed a config with a valid secret key.
    let (signer, _) = load_or_create_identity(&config_path, &HashMap::new()).unwrap();
    let agent = signer.agent_id();

    // A blank/whitespace env override is filtered out → the config seed wins.
    let mut env = HashMap::new();
    env.insert("TINYPLACE_SECRET_KEY".to_string(), "   ".to_string());
    let (again, _) = load_or_create_identity(&config_path, &env).unwrap();
    assert_eq!(again.agent_id(), agent);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn runtime_error_maps_into_sdk_error() {
    use tinyplace::error::Error as SdkError;
    let invalid = super::RuntimeError::Invalid("boom".to_string());
    let sdk: SdkError = invalid.into();
    assert!(matches!(sdk, SdkError::InvalidArgument(_)));

    // An Sdk-wrapped error round-trips back to the same variant.
    let wrapped = super::RuntimeError::Sdk(SdkError::InvalidArgument("x".into()));
    let sdk2: SdkError = wrapped.into();
    assert!(matches!(sdk2, SdkError::InvalidArgument(_)));
}

#[test]
fn load_or_create_identity_is_deterministic_across_calls() {
    let dir = unique_temp_dir("identity");
    let config_path = dir.join("config.json");
    let env: HashMap<String, String> = HashMap::new();

    // First call: no key anywhere, mint and persist.
    let (signer1, config1) = load_or_create_identity(&config_path, &env).unwrap();
    let agent1 = signer1.agent_id();
    let key = config1.secret_key.clone().expect("secret persisted");
    assert_eq!(key.len(), 64, "seed is 64 hex chars");
    assert!(config_path.exists(), "config written");

    // Second call: reads the persisted seed and yields the same identity.
    let (signer2, config2) = load_or_create_identity(&config_path, &env).unwrap();
    assert_eq!(signer2.agent_id(), agent1);
    assert_eq!(config2.secret_key.as_deref(), Some(key.as_str()));

    // An env-provided key overrides the file.
    let other = tinyplace::LocalSigner::generate();
    let other_hex: String = other.seed().iter().map(|b| format!("{b:02x}")).collect();
    let mut env2 = HashMap::new();
    env2.insert("TINYPLACE_SECRET_KEY".to_string(), other_hex.clone());
    let (signer3, config3) = load_or_create_identity(&config_path, &env2).unwrap();
    assert_eq!(signer3.agent_id(), other.agent_id());
    assert_eq!(config3.secret_key.as_deref(), Some(other_hex.as_str()));

    let _ = std::fs::remove_dir_all(&dir);
}
