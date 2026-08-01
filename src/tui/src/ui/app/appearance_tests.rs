//! Tests for Appearance display preferences and their persistence.

use std::sync::Arc;

use medulla::config::{LoadedConfig, TuiConfig};
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;

use super::appearance::{HARNESS_BRANCH_ROW, HARNESS_PATH_ROW};
use super::App;

fn app(path: &std::path::Path) -> App {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults(path.display().to_string()));
    app.set_config_path(path.to_path_buf());
    app
}

fn saved(path: &std::path::Path) -> TuiConfig {
    toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn branch_and_path_toggles_apply_live_and_persist_independently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = app(&path);

    app.appearance_index = HARNESS_BRANCH_ROW;
    app.cycle_appearance_row(true);
    assert!(!app.loaded.config.appearance.show_harness_branch);
    assert!(!saved(&path).appearance.show_harness_branch);
    assert!(saved(&path).appearance.show_harness_path);

    app.appearance_index = HARNESS_PATH_ROW;
    app.cycle_appearance_row(true);
    assert!(!app.loaded.config.appearance.show_harness_path);
    let saved = saved(&path);
    assert!(!saved.appearance.show_harness_branch);
    assert!(!saved.appearance.show_harness_path);
}

#[test]
fn a_toggle_without_a_config_path_still_applies_live() {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("unused.toml".into()));
    app.appearance_index = HARNESS_BRANCH_ROW;

    app.cycle_appearance_row(true);

    assert!(!app.loaded.config.appearance.show_harness_branch);
    assert!(app.status().contains("not persisted"));
}
