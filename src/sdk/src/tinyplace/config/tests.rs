//! Tests for the config module.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::tinyplace::{
    config_path, load_config, parse_config, resolve_endpoint, TinyplaceFileConfig, DEFAULT_ENDPOINT,
};

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn config_path_prefers_env_override() {
    let e = env(&[("TINYPLACE_CONFIG", "/custom/config.json")]);
    assert_eq!(
        config_path(&e, Path::new("/home/me")),
        PathBuf::from("/custom/config.json")
    );
}

#[test]
fn config_path_defaults_under_medulla_home() {
    // No env at all: fall back to the injected home base.
    let e = env(&[]);
    assert_eq!(
        config_path(&e, Path::new("/home/me")),
        PathBuf::from("/home/me/.medulla/tinyplace/config.json")
    );
    // Empty override is ignored.
    let e2 = env(&[("TINYPLACE_CONFIG", "")]);
    assert_eq!(
        config_path(&e2, Path::new("/home/me")),
        PathBuf::from("/home/me/.medulla/tinyplace/config.json")
    );
}

#[test]
fn config_path_follows_the_medulla_home_resolver() {
    // MEDULLA_HOME wins over the injected base.
    let e = env(&[("MEDULLA_HOME", "/explicit/home")]);
    assert_eq!(
        config_path(&e, Path::new("/home/me")),
        PathBuf::from("/explicit/home/tinyplace/config.json")
    );
    // HOME from the environment is used ahead of the fallback base.
    let e2 = env(&[("HOME", "/env/home")]);
    assert_eq!(
        config_path(&e2, Path::new("/home/me")),
        PathBuf::from("/env/home/.medulla/tinyplace/config.json")
    );
    // The identity follows a dev home too.
    let e3 = env(&[("MEDULLA_DEV", "1")]);
    assert_eq!(
        config_path(&e3, Path::new("/home/me")),
        PathBuf::from(".medulla/tinyplace/config.json")
    );
}

#[test]
fn parses_a_full_config() {
    let contents = r#"{
    "endpoint": "https://staging-api.tiny.place",
    "secretKey": "deadbeef",
    "siwsToken": "siws:abc",
    "openHumanOwner": "owner-addr",
    "ignored": true
}"#;
    let config = parse_config(contents);
    assert_eq!(
        config.endpoint.as_deref(),
        Some("https://staging-api.tiny.place")
    );
    assert_eq!(config.secret_key.as_deref(), Some("deadbeef"));
    assert_eq!(config.siws_token.as_deref(), Some("siws:abc"));
    assert_eq!(config.open_human_owner.as_deref(), Some("owner-addr"));
}

#[test]
fn parse_config_tolerates_junk() {
    assert_eq!(parse_config("not json"), TinyplaceFileConfig::default());
    assert_eq!(parse_config("[1,2,3]"), TinyplaceFileConfig::default());
    assert_eq!(parse_config("42"), TinyplaceFileConfig::default());
    assert_eq!(parse_config("{}"), TinyplaceFileConfig::default());
}

#[test]
fn load_config_missing_file_is_empty() {
    let config = load_config(Path::new("/no/such/tinyplace/config.json"));
    assert_eq!(config, TinyplaceFileConfig::default());
}

#[test]
fn load_config_reads_a_real_file() {
    let dir = std::env::temp_dir().join(format!("tinyplace-proto-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.json");
    std::fs::write(
        &path,
        r#"{"endpoint":"https://x.example","secretKey":"ab"}"#,
    )
    .unwrap();
    let config = load_config(&path);
    assert_eq!(config.endpoint.as_deref(), Some("https://x.example"));
    assert_eq!(config.secret_key.as_deref(), Some("ab"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn round_trips_config_omitting_empty_fields() {
    let config = TinyplaceFileConfig {
        endpoint: Some("https://x".to_string()),
        secret_key: None,
        siws_token: None,
        open_human_owner: None,
    };
    let json = serde_json::to_string(&config).unwrap();
    assert_eq!(json, r#"{"endpoint":"https://x"}"#);
    assert_eq!(parse_config(&json), config);
}

#[test]
fn endpoint_resolution_order() {
    let config = TinyplaceFileConfig {
        endpoint: Some("https://config-endpoint".to_string()),
        ..Default::default()
    };

    // TINYPLACE_ENDPOINT wins over everything.
    let e = env(&[
        ("TINYPLACE_ENDPOINT", "https://one"),
        ("TINYPLACE_API_URL", "https://two"),
        ("NEXT_PUBLIC_API_URL", "https://three"),
    ]);
    assert_eq!(resolve_endpoint(&e, &config), "https://one");

    // Then TINYPLACE_API_URL.
    let e = env(&[
        ("TINYPLACE_API_URL", "https://two"),
        ("NEXT_PUBLIC_API_URL", "https://three"),
    ]);
    assert_eq!(resolve_endpoint(&e, &config), "https://two");

    // Then NEXT_PUBLIC_API_URL.
    let e = env(&[("NEXT_PUBLIC_API_URL", "https://three")]);
    assert_eq!(resolve_endpoint(&e, &config), "https://three");

    // Then config.endpoint.
    let e = env(&[]);
    assert_eq!(resolve_endpoint(&e, &config), "https://config-endpoint");

    // Finally the default.
    assert_eq!(
        resolve_endpoint(&e, &TinyplaceFileConfig::default()),
        DEFAULT_ENDPOINT
    );
}

#[test]
fn empty_env_values_are_skipped() {
    let config = TinyplaceFileConfig::default();
    let e = env(&[
        ("TINYPLACE_ENDPOINT", ""),
        ("TINYPLACE_API_URL", "https://real"),
    ]);
    assert_eq!(resolve_endpoint(&e, &config), "https://real");
}
