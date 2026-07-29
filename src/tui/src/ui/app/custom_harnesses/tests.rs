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

#[test]
fn editor_preserves_inherited_presets_when_writing_the_project_layer() {
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let global = home.path().join("config.toml");
    let project_config = project.path().join("medulla.toml");
    std::fs::write(
        &global,
        r#"
[[customHarnesses]]
id = "global"
name = "Global"
baseHarness = "claude"
model = "openrouter/global"
hostId = "this-device"
"#,
    )
    .unwrap();
    std::fs::write(&project_config, "[theme]\naccent = \"blue\"\n").unwrap();
    let env = std::collections::HashMap::from([(
        "MEDULLA_HOME".to_string(),
        home.path().to_string_lossy().into_owned(),
    )]);
    let loaded = medulla::config::load_config(None, &env, project.path()).unwrap();
    let mut app = App::new(Arc::new(MockRuntime::demo()), loaded);
    app.set_config_path(project_config.clone());

    assert_eq!(app.custom_harnesses.len(), 1);
    assert_eq!(app.custom_harnesses[0].id, "global");

    app.save_custom_harness(
        None,
        "project | Project | codex | openrouter/project | | this-device",
    );

    let saved = medulla::config::load_custom_harnesses(&project_config).unwrap();
    let ids: Vec<_> = saved.iter().map(|harness| harness.id.as_str()).collect();
    assert_eq!(ids, vec!["global", "project"]);
}
