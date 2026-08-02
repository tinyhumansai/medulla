//! Unit tests for layered config discovery, parsing, merging, and env overrides.

use super::load::merge_value;
use super::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The account home inside a `MEDULLA_HOME` root: `MEDULLA_HOME` names the
/// directory that holds accounts, and a test signs nobody in, so every path the
/// loader derives lands under the pre-login account.
fn home_of(root: &Path) -> PathBuf {
    root.join(crate::home::user::PRE_LOGIN_USER_ID)
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
        home_of(&home).join("state").to_string_lossy()
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
        home_of(&home).join("state").to_string_lossy()
    );
    assert_eq!(
        loaded.config.tinyplace.unwrap().identity_dir,
        home_of(&home).join("tinyplace").to_string_lossy()
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
    std::fs::create_dir_all(home_of(&home)).unwrap();
    std::fs::write(
        home_of(&home).join("config.toml"),
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
    // `sources` is ordered low → high precedence: global first, project-local
    // last. Write-backs (routing strategy, onboarding) target `sources.last()`,
    // so the file that wins on reload is the one written — pin that ordering.
    assert!(
        loaded.sources[0].ends_with("config.toml") && loaded.sources[0].contains("layer-home"),
        "sources[0] is the global config: {:?}",
        loaded.sources
    );
    assert!(
        loaded.sources[1].contains(".medulla"),
        "sources.last() is the highest-precedence project-local config: {:?}",
        loaded.sources
    );

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
fn load_config_layers_router_section_from_toml() {
    // A global config sets the top-level router endpoint + key-var name; a
    // project-local config adds a per-provider (claude) Anthropic-passthrough
    // override. The field-level merge keeps both, and precedence resolves so
    // claude uses its override while codex inherits the top-level endpoint.
    let home = temp_dir("router-home");
    let cwd = temp_dir("router-cwd");
    std::fs::create_dir_all(home_of(&home)).unwrap();
    std::fs::write(
        home_of(&home).join("config.toml"),
        "[router]\nbaseUrl = \"https://gateway.internal/v1\"\napiKeyEnv = \"MEDULLA_ROUTER_KEY\"\n\n[router.models]\nreasoning = \"gpt-tier-a\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(cwd.join(".medulla")).unwrap();
    std::fs::write(
        cwd.join(".medulla").join("config.toml"),
        "[router.providers.claude]\nbaseUrl = \"https://gateway.internal/anthropic\"\n",
    )
    .unwrap();

    let loaded = load_config(
        None,
        &env(&[("MEDULLA_HOME", home.to_str().unwrap())]),
        &cwd,
    )
    .unwrap();
    let router = loaded.config.router.expect("router section merged");
    // Global top-level survives the merge.
    assert_eq!(router.api_key_env.as_deref(), Some("MEDULLA_ROUTER_KEY"));
    assert_eq!(router.model_for_tier("reasoning"), Some("gpt-tier-a"));
    // Project-local provider override wins for claude; codex inherits top-level.
    assert_eq!(
        router.base_url_for("claude"),
        Some("https://gateway.internal/anthropic")
    );
    assert_eq!(
        router.base_url_for("codex"),
        Some("https://gateway.internal/v1")
    );
    assert_eq!(loaded.sources.len(), 2);
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
    // Built with `join` rather than written out: the value is a real path, so
    // on Windows it comes back separated with a backslash.
    let expected = std::path::Path::new("/tmp/mh")
        .join(crate::home::user::PRE_LOGIN_USER_ID)
        .join("tinyplace")
        .to_string_lossy()
        .into_owned();
    assert_eq!(default_tinyplace_config(&prod).identity_dir, expected);
}

#[test]
fn explicit_config_from_env_reads_the_path_a_parent_process_recorded() {
    // A subprocess (the `medulla workflow mcp` tool server, an ACP harness)
    // only inherits environment, not the parent's parsed `--config` flag. This
    // is the one place that env var is read back, so every subprocess call
    // site agrees on how to find it.
    let with_path = env(&[(CONFIG_PATH_ENV, "/tmp/explicit.toml")]);
    assert_eq!(
        explicit_config_from_env(&with_path),
        Some("/tmp/explicit.toml")
    );
}

#[test]
fn explicit_config_from_env_is_none_when_the_parent_never_set_it() {
    // The common case — the TUI was launched without `--config` — must not
    // manufacture a path that then fails to open.
    assert_eq!(explicit_config_from_env(&HashMap::new()), None);
}

#[test]
fn a_subprocess_reading_the_recorded_path_loads_the_same_config_the_parent_did() {
    // The end-to-end claim the fix makes: a subprocess that resolves its
    // config via `explicit_config_from_env` sees the exact file the parent's
    // own `--config` pointed at, not whatever `config_file_layers` would have
    // discovered from `cwd` on its own. Before the fix, every such subprocess
    // passed `None` unconditionally and could silently answer with a more
    // permissive default config (e.g. `allowCode = true`) than the one the
    // operator explicitly chose.
    let home = temp_dir("subprocess-home");
    let discoverable_cwd = temp_dir("subprocess-cwd");
    // A config file `cwd` would happily discover if nothing overrode it —
    // permissive, so the test fails loudly if the override is ignored.
    std::fs::write(
        discoverable_cwd.join("medulla.toml"),
        "[workflows]\nallowCode = true\n",
    )
    .unwrap();

    // The explicit file the parent process actually chose — restrictive, and
    // in a different directory than `cwd` so nothing but the env var can find
    // it.
    let explicit_dir = temp_dir("subprocess-explicit");
    let explicit_path = explicit_dir.join("chosen.toml");
    std::fs::write(&explicit_path, "[workflows]\nallowCode = false\n").unwrap();

    let mut parent_env = env(&[("MEDULLA_HOME", home.to_str().unwrap())]);
    parent_env.insert(
        CONFIG_PATH_ENV.to_string(),
        explicit_path.to_string_lossy().into_owned(),
    );

    let loaded = load_config(
        explicit_config_from_env(&parent_env),
        &parent_env,
        &discoverable_cwd,
    )
    .unwrap();

    assert!(
        !loaded.config.workflows.allow_code,
        "the subprocess must load the explicit config the parent recorded, \
         not silently discover the more permissive one sitting in its cwd"
    );
}
