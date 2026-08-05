//! Unit tests for the Codex config overrides and the derived model catalog.

use std::collections::HashMap;
use std::error::Error as _;

use serde_json::{json, Value};

use super::*;
use crate::protocol::HarnessProvider;

/// An environment with a routed endpoint, the preset opt-in, and homes pointed
/// at `dir` so nothing reads or writes the developer's own files.
fn routed_env(dir: &std::path::Path) -> HashMap<String, String> {
    HashMap::from([
        (
            "OPENAI_BASE_URL".to_string(),
            "http://127.0.0.1:7777/openai".to_string(),
        ),
        (OVERRIDES_ENV.to_string(), "1".to_string()),
        (
            "CODEX_HOME".to_string(),
            dir.join("codex").display().to_string(),
        ),
        (
            "MEDULLA_HOME".to_string(),
            dir.join("medulla").display().to_string(),
        ),
        ("HOME".to_string(), dir.display().to_string()),
    ])
}

/// A minimal stand-in for the catalog Codex caches for itself.
fn write_codex_cache(dir: &std::path::Path) {
    let home = dir.join("codex");
    std::fs::create_dir_all(&home).unwrap();
    let cache = json!({
        "models": [
            {
                "slug": "codex-auto-review",
                "priority": 0,
                "context_window": 100,
                "apply_patch_tool_type": "freeform",
            },
            {
                "slug": "gpt-5.6-sol",
                "display_name": "GPT-5.6",
                "priority": 2,
                "context_window": 400_000,
                "instructions": "be a good coding agent",
                "apply_patch_tool_type": "freeform",
                "tool_mode": "code_mode_only",
                "supports_search_tool": true,
                "shell_type": "shell_command",
                "upgrade": {"slug": "gpt-9"},
            },
            {
                "slug": "gpt-5.4",
                "priority": 5,
                "context_window": 200_000,
            },
        ]
    });
    std::fs::write(home.join("models_cache.json"), cache.to_string()).unwrap();
}

/// The `-c` pairs as a map, so assertions name a key rather than an index.
fn overrides(args: &[String]) -> HashMap<String, String> {
    let mut pairs = HashMap::new();
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        assert_eq!(flag, "-c", "every override is introduced by -c");
        let assignment = rest.next().expect("-c is followed by an assignment");
        let (key, value) = assignment.split_once('=').expect("assignment has a value");
        pairs.insert(key.to_string(), value.trim_matches('"').to_string());
    }
    pairs
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("medulla-codex-overrides-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_routed_codex_preset_gets_a_provider_block_and_a_catalog() {
    let dir = scratch("full");
    write_codex_cache(&dir);
    let env = routed_env(&dir);

    let args = launch_args(HarnessProvider::Codex, Some("deepseek/deepseek-v4"), &env).unwrap();
    let pairs = overrides(&args);

    assert_eq!(pairs.get("model_provider").unwrap(), PROVIDER_ID);
    assert_eq!(
        pairs
            .get(&format!("model_providers.{PROVIDER_ID}.base_url"))
            .unwrap(),
        "http://127.0.0.1:7777/openai"
    );
    assert_eq!(
        pairs
            .get(&format!("model_providers.{PROVIDER_ID}.env_key"))
            .unwrap(),
        "OPENAI_API_KEY"
    );
    assert_eq!(
        pairs
            .get(&format!("model_providers.{PROVIDER_ID}.wire_api"))
            .unwrap(),
        "responses"
    );
    // Without this the signed-in ChatGPT account wins and the block is ignored.
    assert_eq!(pairs.get("preferred_auth_method").unwrap(), "apikey");
    assert_eq!(pairs.get("model_reasoning_effort").unwrap(), DEFAULT_EFFORT);

    let catalog: Value =
        serde_json::from_slice(&std::fs::read(pairs.get("model_catalog_json").unwrap()).unwrap())
            .unwrap();
    let entry = &catalog["models"][0];
    assert_eq!(entry["slug"], json!("deepseek/deepseek-v4"));
    // Derived from the highest-priority non-review model, so Codex's own
    // instructions come along.
    assert_eq!(entry["instructions"], json!("be a good coding agent"));
    assert_eq!(entry["context_window"], json!(DEFAULT_CONTEXT_WINDOW));
    // The three shapes a function-only provider rejects outright.
    assert_eq!(entry["apply_patch_tool_type"], Value::Null);
    assert_eq!(entry["tool_mode"], Value::Null);
    assert_eq!(entry["supports_search_tool"], json!(false));
    // The richer shell tool is a plain function and is left alone.
    assert_eq!(entry["shell_type"], json!("shell_command"));
    // Nothing may point at a model this catalog does not contain.
    assert_eq!(entry["upgrade"], Value::Null);
}

#[test]
fn the_preset_chooses_the_effort_window_and_name() {
    let dir = scratch("knobs");
    write_codex_cache(&dir);
    let mut env = routed_env(&dir);
    env.insert(EFFORT_ENV.to_string(), "medium".to_string());
    env.insert(CONTEXT_WINDOW_ENV.to_string(), "128000".to_string());
    env.insert(
        DISPLAY_NAME_ENV.to_string(),
        "DeepSeek via Codex".to_string(),
    );

    let args = launch_args(HarnessProvider::Codex, Some("deepseek/deepseek-v4"), &env).unwrap();
    let pairs = overrides(&args);
    assert_eq!(pairs.get("model_reasoning_effort").unwrap(), "medium");

    let catalog: Value =
        serde_json::from_slice(&std::fs::read(pairs.get("model_catalog_json").unwrap()).unwrap())
            .unwrap();
    assert_eq!(catalog["models"][0]["context_window"], json!(128_000));
    assert_eq!(
        catalog["models"][0]["display_name"],
        json!("DeepSeek via Codex")
    );
}

#[test]
fn nothing_is_injected_without_the_opt_in_the_endpoint_or_codex() {
    let dir = scratch("gates");
    write_codex_cache(&dir);

    let mut without_opt_in = routed_env(&dir);
    without_opt_in.remove(OVERRIDES_ENV);
    assert!(
        launch_args(HarnessProvider::Codex, Some("m"), &without_opt_in)
            .unwrap()
            .is_empty()
    );

    let mut unrouted = routed_env(&dir);
    unrouted.remove("OPENAI_BASE_URL");
    assert!(launch_args(HarnessProvider::Codex, Some("m"), &unrouted)
        .unwrap()
        .is_empty());

    // Another harness never sees Codex's config keys, opted in or not.
    assert!(
        launch_args(HarnessProvider::Claude, Some("m"), &routed_env(&dir))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_run_with_no_model_still_gets_the_provider_block() {
    let dir = scratch("no-model");
    write_codex_cache(&dir);

    let args = launch_args(HarnessProvider::Codex, None, &routed_env(&dir)).unwrap();
    let pairs = overrides(&args);
    assert_eq!(pairs.get("model_provider").unwrap(), PROVIDER_ID);
    // There is no slug to key a catalog entry on, so none is written.
    assert!(!pairs.contains_key("model_catalog_json"));
}

#[test]
fn a_missing_codex_cache_names_what_to_run() {
    let dir = scratch("no-cache");
    let error = launch_args(HarnessProvider::Codex, Some("m"), &routed_env(&dir))
        .unwrap_err()
        .to_string();
    assert!(error.contains("models_cache.json"), "{error}");
    assert!(error.contains("Run `codex` once"), "{error}");
}

#[test]
fn a_slug_with_a_slash_stays_one_file() {
    let env = HashMap::from([("MEDULLA_HOME".to_string(), "/tmp/medulla".to_string())]);
    let path = catalog_path("deepseek/deepseek-v4-flash-0731", "DeepSeek", 128_000, &env);
    let file_name = path.file_name().and_then(|name| name.to_str()).unwrap();
    assert!(
        file_name.starts_with("deepseek_deepseek-v4-flash-0731-"),
        "{file_name}"
    );
    assert!(file_name.ends_with(".json"), "{file_name}");
    assert_eq!(
        path.parent().unwrap(),
        std::path::Path::new("/tmp/medulla/codex-catalogs")
    );
}

#[test]
fn different_presets_for_the_same_model_slug_never_share_a_catalog_file() {
    // Regression test: `catalog_path` used to be keyed on the sanitized model
    // slug alone, so two presets routing the same slug with a different
    // display name or context window (or two slugs that sanitize to the same
    // file name) shared one file. A later spawn's rename could then replace
    // an earlier spawn's catalog after its `-c model_catalog_json=<path>`
    // argv was already handed to Codex but before Codex read it.
    let env = HashMap::from([("MEDULLA_HOME".to_string(), "/tmp/medulla".to_string())]);
    let same_name_and_window = catalog_path("deepseek/deepseek-v4", "DeepSeek", 128_000, &env);
    let different_window = catalog_path("deepseek/deepseek-v4", "DeepSeek", 64_000, &env);
    let different_name = catalog_path("deepseek/deepseek-v4", "DeepSeek (fast)", 128_000, &env);
    // Two slugs that sanitize to the same file-name stem.
    let colliding_slug = catalog_path("deepseek_v4", "DeepSeek", 128_000, &env);

    assert_ne!(same_name_and_window, different_window);
    assert_ne!(same_name_and_window, different_name);
    assert_ne!(different_window, different_name);
    assert_ne!(same_name_and_window, colliding_slug);

    // An identical derivation still reuses the same path, so repeatedly
    // launching one preset does not proliferate catalog files.
    assert_eq!(
        same_name_and_window,
        catalog_path("deepseek/deepseek-v4", "DeepSeek", 128_000, &env)
    );
}

#[test]
fn two_concurrent_writers_for_the_same_model_both_succeed() {
    // Regression test for a race where two spawns for the same model derived
    // the same deterministic `*.json.tmp` name: whichever renamed second found
    // its own temporary file already gone and failed, even though the shared
    // catalog the first writer produced was perfectly valid.
    let dir = scratch("concurrent");
    write_codex_cache(&dir);
    let env = std::sync::Arc::new(routed_env(&dir));

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let env = std::sync::Arc::clone(&env);
            std::thread::spawn(move || {
                write_catalog(
                    "deepseek/deepseek-v4",
                    "DeepSeek",
                    DEFAULT_CONTEXT_WINDOW,
                    &env,
                )
            })
        })
        .collect();

    for handle in handles {
        handle
            .join()
            .expect("writer thread did not panic")
            .expect("concurrent catalog write must not fail");
    }

    let path = catalog_path(
        "deepseek/deepseek-v4",
        "DeepSeek",
        DEFAULT_CONTEXT_WINDOW,
        &env,
    );
    let catalog: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(catalog["models"][0]["slug"], json!("deepseek/deepseek-v4"));
    // No leftover temporary files: every writer's own unique name was renamed
    // or cleaned up, never left behind under the model's catalog directory.
    let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn write_catalog_errors_are_actionable() {
    let dir = scratch("errors");
    let env = routed_env(&dir);

    let missing_cache = write_catalog("m", "M", DEFAULT_CONTEXT_WINDOW, &env).unwrap_err();
    assert!(missing_cache.to_string().contains("models_cache.json"));
    assert!(missing_cache.source().is_some());

    let home = dir.join("codex");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("models_cache.json"), "not json").unwrap();
    let invalid_json = write_catalog("m", "M", DEFAULT_CONTEXT_WINDOW, &env).unwrap_err();
    assert!(invalid_json.to_string().contains("not valid JSON"));

    std::fs::write(
        home.join("models_cache.json"),
        json!({"models": []}).to_string(),
    )
    .unwrap();
    let no_template = write_catalog("m", "M", DEFAULT_CONTEXT_WINDOW, &env).unwrap_err();
    assert!(no_template.to_string().contains("no usable model"));
}

#[test]
fn a_quote_in_a_value_cannot_break_out_of_the_toml_string() {
    let dir = scratch("quoting");
    write_codex_cache(&dir);
    let mut env = routed_env(&dir);
    env.insert(
        "OPENAI_BASE_URL".to_string(),
        "http://127.0.0.1/\"; evil = \"1".to_string(),
    );

    let args = launch_args(HarnessProvider::Codex, None, &env).unwrap();
    let assignment = args
        .iter()
        .find(|arg| arg.starts_with(&format!("model_providers.{PROVIDER_ID}.base_url=")))
        .unwrap();
    let value = assignment.split_once('=').unwrap().1;
    let parsed: toml::Value = toml::from_str(&format!("value = {value}")).unwrap();
    assert_eq!(
        parsed["value"].as_str().unwrap(),
        "http://127.0.0.1/\"; evil = \"1"
    );
    assert!(parsed.as_table().unwrap().get("evil").is_none());
}
