//! Deterministic tests for event-loop state refresh and update checks.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::App;

use super::should_refresh_context;
use super::update_checker::spawn_update_checker;

#[test]
fn context_refresh_tracks_the_nested_settings_page() {
    let runtime = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));

    let _ = app.focus_settings_subpage("Usage");
    assert!(!should_refresh_context(&mut app));
    let _ = app.focus_settings_subpage("Context");
    assert!(should_refresh_context(&mut app));
    assert!(!should_refresh_context(&mut app));
}

#[test]
fn disabled_update_check_spawns_no_background_work() {
    let dir = tempfile::tempdir().unwrap();
    let env = std::collections::HashMap::new();
    let mut loaded = medulla::config::load_config(None, &env, dir.path()).unwrap();
    loaded.config.update.check = false;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    spawn_update_checker(&loaded, &tx);

    assert!(rx.try_recv().is_err());
}
