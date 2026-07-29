//! Focused tests for custom harness editing and secret-free persistence.

use std::sync::Arc;

use crate::ui::app::types::App;
use medulla::runtime::mock::MockRuntime;

fn app_with_config(path: &std::path::Path) -> App {
    let loaded = medulla::config::LoadedConfig::defaults(path.display().to_string());
    let mut app = App::new(Arc::new(MockRuntime::demo()), loaded);
    app.set_config_path(path.to_path_buf());
    app
}

#[test]
fn adds_edits_and_deletes_a_preset_without_persisting_the_key_value() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[theme]\naccent = \"blue\"\n").expect("seed config");
    let mut app = app_with_config(&path);

    app.save_custom_harness(
        None,
        "deepseek | DeepSeek | claude | deepseek/deepseek-chat | | this-device",
    );
    assert_eq!(app.custom_harnesses.len(), 1);
    assert!(app.status().contains("restart host"));

    app.save_custom_harness(
        Some("deepseek"),
        "deepseek-codex | DeepSeek Codex | codex | deepseek/deepseek-chat | | this-device",
    );
    assert_eq!(app.custom_harnesses[0].id, "deepseek-codex");

    let saved = std::fs::read_to_string(&path).expect("saved config");
    assert!(saved.contains("deepseek-codex"));
    assert!(saved.contains("accent = \"blue\""));
    assert!(!saved.contains("sk-or-"));

    app.delete_selected_custom_harness();
    assert!(app.custom_harnesses.is_empty());
    assert!(medulla::config::load_custom_harnesses(&path)
        .expect("reload")
        .is_empty());
}

#[test]
fn duplicate_ids_are_rejected_without_overwriting_the_existing_preset() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let mut app = app_with_config(&path);
    let line = "deepseek | DeepSeek | claude | deepseek/deepseek-chat | | this-device";

    app.save_custom_harness(None, line);
    app.save_custom_harness(None, line);

    assert_eq!(app.custom_harnesses.len(), 1);
    assert!(app.status().contains("already exists"));
}
