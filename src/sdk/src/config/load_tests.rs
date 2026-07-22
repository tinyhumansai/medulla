//! Unit tests for layered config discovery, parsing, merging, and env overrides.

use super::load::merge_value;
use super::*;
use std::collections::HashMap;
use std::path::PathBuf;

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// A unique temp dir for a test, used as an injected `MEDULLA_HOME` and/or cwd.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "medulla-cfg-{tag}-{}-{:p}",
        std::process::id(),
        &tag as *const _
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn load_config_applies_staging_switch_to_both_urls() {
    let home = temp_dir("staging-home");
    let cwd = temp_dir("staging-cwd");
    let base_env = &[
        ("MEDULLA_HOME", home.to_str().unwrap()),
        ("MEDULLA_STAGING", "1"),
    ];
    // No config file + staging env → staging defaults for backend.
    let loaded = load_config(None, &env(base_env), &cwd).unwrap();
    assert_eq!(
        loaded.config.backend.base_url,
        "https://staging-api.tinyhumans.ai"
    );

    let cfg = cwd.join("medulla.tui.json");
    std::fs::write(&cfg, r#"{"tinyplace":{"peers":[]}}"#).unwrap();
    let loaded = load_config(Some(cfg.to_str().unwrap()), &env(base_env), &cwd).unwrap();
    assert_eq!(
        loaded.config.backend.base_url,
        "https://staging-api.tinyhumans.ai"
    );
    assert_eq!(
        loaded.config.tinyplace.unwrap().base_url,
        "https://staging-api.tiny.place"
    );
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn load_config_explicit_urls_win_over_env() {
    let home = temp_dir("explicit-home");
    let cwd = temp_dir("explicit-cwd");
    let cfg = cwd.join("medulla.tui.json");
    std::fs::write(
        &cfg,
        r#"{"backend":{"baseUrl":"http://be:1"},"tinyplace":{"baseUrl":"http://tp:2","peers":[]}}"#,
    )
    .unwrap();
    let home_env = ("MEDULLA_HOME", home.to_str().unwrap());
    // Staging set, but explicit config baseUrls win.
    let loaded = load_config(
        Some(cfg.to_str().unwrap()),
        &env(&[home_env, ("MEDULLA_STAGING", "1")]),
        &cwd,
    )
    .unwrap();
    assert_eq!(loaded.config.backend.base_url, "http://be:1");
    assert_eq!(loaded.config.tinyplace.unwrap().base_url, "http://tp:2");
    // But MEDULLA_API_URL still beats an explicit backend baseUrl.
    let loaded = load_config(
        Some(cfg.to_str().unwrap()),
        &env(&[home_env, ("MEDULLA_API_URL", "http://env:9")]),
        &cwd,
    )
    .unwrap();
    assert_eq!(loaded.config.backend.base_url, "http://env:9");
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn load_config_missing_file_yields_home_derived_defaults() {
    let home = temp_dir("nope-home");
    let cwd = temp_dir("nope-cwd");
    // No files anywhere → defaults, with state dir under <home>/state.
    let loaded = load_config(
        None,
        &env(&[("MEDULLA_HOME", home.to_str().unwrap())]),
        &cwd,
    )
    .unwrap();
    assert_eq!(
        loaded.config.state_dir,
        home.join("state").to_string_lossy()
    );
    assert_eq!(loaded.path, "(built-in defaults)");
    assert!(loaded.sources.is_empty());
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn load_config_reads_and_parses_a_file() {
    let home = temp_dir("reads-home");
    let dir = temp_dir("reads-cwd");
    let path = dir.join("medulla.tui.json");
    std::fs::write(&path, r#"{"stateDir":"/custom/state"}"#).unwrap();
    let loaded = load_config(
        Some(path.to_str().unwrap()),
        &env(&[("MEDULLA_HOME", home.to_str().unwrap())]),
        &dir,
    )
    .unwrap();
    // An explicit stateDir is preserved (not overridden by <home>/state).
    assert_eq!(loaded.config.state_dir, "/custom/state");
    assert_eq!(loaded.sources.len(), 1);
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_config_invalid_json_errors() {
    let dir = temp_dir("bad-cwd");
    let path = dir.join("bad.json");
    std::fs::write(&path, "{ this is not json").unwrap();
    let err = load_config(Some(path.to_str().unwrap()), &env(&[]), &dir).unwrap_err();
    assert!(err.to_string().contains("Invalid JSON"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_config_state_and_identity_derive_from_home() {
    let home = temp_dir("derive-home");
    let cwd = temp_dir("derive-cwd");
    // A tinyplace section with no identityDir → <home>/tinyplace; stateDir → <home>/state.
    let cfg = cwd.join("medulla.toml");
    std::fs::write(&cfg, "[tinyplace]\npeers = []\n").unwrap();
    let loaded = load_config(
        None,
        &env(&[("MEDULLA_HOME", home.to_str().unwrap())]),
        &cwd,
    )
    .unwrap();
    assert_eq!(
        loaded.config.state_dir,
        home.join("state").to_string_lossy()
    );
    assert_eq!(
        loaded.config.tinyplace.unwrap().identity_dir,
        home.join("tinyplace").to_string_lossy()
    );
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn load_config_state_dir_env_override_wins() {
    let home = temp_dir("stateenv-home");
    let cwd = temp_dir("stateenv-cwd");
    let loaded = load_config(
        None,
        &env(&[
            ("MEDULLA_HOME", home.to_str().unwrap()),
            ("MEDULLA_STATE_DIR", "/env/state"),
        ]),
        &cwd,
    )
    .unwrap();
    assert_eq!(loaded.config.state_dir, "/env/state");
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn load_config_layers_global_project_env_flag() {
    let home = temp_dir("layer-home");
    let cwd = temp_dir("layer-cwd");
    // Global config sets a base URL and a token env name.
    std::fs::write(
        home.join("config.toml"),
        "[backend]\nbaseUrl = \"http://global:1\"\ntokenEnv = \"GLOBAL_TOK\"\n",
    )
    .unwrap();
    // Project-local overrides just backend.baseUrl (field-level merge).
    std::fs::create_dir_all(cwd.join(".medulla")).unwrap();
    std::fs::write(
        cwd.join(".medulla").join("config.toml"),
        "[backend]\nbaseUrl = \"http://project:2\"\n",
    )
    .unwrap();

    // Global < project: project wins on baseUrl, global's tokenEnv survives.
    let loaded = load_config(
        None,
        &env(&[("MEDULLA_HOME", home.to_str().unwrap())]),
        &cwd,
    )
    .unwrap();
    assert_eq!(loaded.config.backend.base_url, "http://project:2");
    assert_eq!(loaded.config.backend.token_env, "GLOBAL_TOK");
    assert_eq!(loaded.sources.len(), 2);

    // Env beats both files.
    let loaded = load_config(
        None,
        &env(&[
            ("MEDULLA_HOME", home.to_str().unwrap()),
            ("MEDULLA_API_URL", "http://env:3"),
        ]),
        &cwd,
    )
    .unwrap();
    assert_eq!(loaded.config.backend.base_url, "http://env:3");
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn load_config_toml_and_json_parity() {
    let home = temp_dir("parity-home");
    let cwd = temp_dir("parity-cwd");
    let home_env = ("MEDULLA_HOME", home.to_str().unwrap());
    let json = cwd.join("c.json");
    std::fs::write(
        &json,
        r#"{"backend":{"baseUrl":"http://x:1"},"medulla":{"maxPasses":3}}"#,
    )
    .unwrap();
    let toml_path = cwd.join("c.toml");
    std::fs::write(
        &toml_path,
        "[backend]\nbaseUrl = \"http://x:1\"\n\n[medulla]\nmaxPasses = 3\n",
    )
    .unwrap();
    let from_json = load_config(Some(json.to_str().unwrap()), &env(&[home_env]), &cwd).unwrap();
    let from_toml =
        load_config(Some(toml_path.to_str().unwrap()), &env(&[home_env]), &cwd).unwrap();
    assert_eq!(from_json.config.backend.base_url, "http://x:1");
    assert_eq!(from_toml.config.backend.base_url, "http://x:1");
    assert_eq!(from_json.config.medulla.max_passes, Some(3));
    assert_eq!(from_toml.config.medulla.max_passes, Some(3));
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn merge_value_is_recursive() {
    let mut base = serde_json::json!({"a":{"x":1,"y":2},"b":9});
    merge_value(&mut base, serde_json::json!({"a":{"y":5,"z":3},"c":7}));
    assert_eq!(
        base,
        serde_json::json!({"a":{"x":1,"y":5,"z":3},"b":9,"c":7})
    );
}

#[test]
fn a_synthesized_tinyplace_section_honours_the_staging_switch() {
    // Regression. `medulla daemon --tui` synthesizes this section when the
    // config file has none, and used to do it with `TinyplaceConfig::default()`
    // — whose `base_url` is the *constant* prod relay, because a serde default
    // cannot read the environment. Under `MEDULLA_STAGING=1` that put the worker
    // on prod while the orchestrator's hub (which resolves from env) sat on
    // staging: both started cleanly, published keys, and reported healthy, but a
    // contact request sent on one relay does not exist on the other, so the
    // worker's Requests tab stayed empty forever with nothing logged anywhere.
    let staging = env(&[("MEDULLA_STAGING", "1"), ("MEDULLA_HOME", "/tmp/mh")]);
    assert_eq!(
        default_tinyplace_config(&staging).base_url,
        "https://staging-api.tiny.place",
        "the synthesized section must follow MEDULLA_STAGING, not a constant"
    );
    assert_ne!(
        TinyplaceConfig::default().base_url,
        default_tinyplace_config(&staging).base_url,
        "if these ever agree this test has stopped proving anything"
    );

    // Absent the switch it is still prod, and the identity dir is home-derived
    // either way — the same wallet `medulla daemon` would have used.
    let prod = env(&[("MEDULLA_HOME", "/tmp/mh")]);
    assert_eq!(
        default_tinyplace_config(&prod).base_url,
        "https://api.tiny.place"
    );
    assert_eq!(
        default_tinyplace_config(&prod).identity_dir,
        "/tmp/mh/tinyplace"
    );
}
