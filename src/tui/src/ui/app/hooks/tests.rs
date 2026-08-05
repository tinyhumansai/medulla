//! Focused tests for hook editing: what reaches the operator's config file,
//! and what is refused rather than silently discarded.

use std::sync::Arc;

use medulla::harness_hooks::{HookEvent, HooksConfig};
use medulla::runtime::mock::MockRuntime;

use crate::ui::app::types::App;

fn app_with_config(path: &std::path::Path) -> App {
    let loaded = medulla::config::LoadedConfig::defaults(path.display().to_string());
    let mut app = App::new(Arc::new(MockRuntime::demo()), loaded);
    app.set_config_path(path.to_path_buf());
    app
}

/// An app whose loaded config already carries Medulla's own hooks, as a real
/// load would leave it.
fn app_with_builtins(path: &std::path::Path) -> App {
    let mut app = app_with_config(path);
    app.loaded.config.hooks = HooksConfig::default()
        .with_builtin(medulla::harness_hooks::builtin::hooks("/usr/bin/medulla"));
    app
}

#[test]
fn adds_edits_and_removes_a_hook_without_touching_unrelated_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[theme]\naccent = \"blue\"\n").expect("seed config");
    let mut app = app_with_config(&path);

    app.save_hook(None, "PostToolUse | Edit,Write |  | 30 | ./bin/auto-commit");
    assert_eq!(app.hook_rows().len(), 1);
    let hook = &app.hook_rows()[0];
    assert_eq!(hook.event, HookEvent::PostToolUse);
    // Commas are the editor's spelling of the harnesses' own alternation.
    assert_eq!(hook.matcher, "Edit|Write");
    assert_eq!(hook.timeout(), Some(30));

    app.save_hook(Some(0), "Stop |  | claude |  | notify-send done");
    assert_eq!(app.hook_rows()[0].event, HookEvent::Stop);

    let saved = std::fs::read_to_string(&path).expect("saved config");
    assert!(saved.contains("notify-send done"));
    assert!(saved.contains("accent = \"blue\""));

    app.delete_selected_hook();
    assert!(app.hook_rows().is_empty());
    let saved = std::fs::read_to_string(&path).expect("saved config");
    assert!(!saved.contains("notify-send done"));
    assert!(saved.contains("accent = \"blue\""));
}

#[test]
fn a_malformed_line_is_reported_and_changes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let mut app = app_with_config(&path);

    app.save_hook(None, "Whenever | | | | do-something");
    assert!(app.hook_rows().is_empty());
    assert!(app.status().contains("not a lifecycle event"));

    app.save_hook(None, "Stop | | | | ");
    assert!(app.hook_rows().is_empty());
    assert!(app.status().contains("needs a command"));
}

#[test]
fn medullas_own_hooks_are_never_written_to_the_operators_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let mut app = app_with_builtins(&path);
    let builtins = app.hook_rows().len();

    app.save_hook(None, "Stop |  |  |  | notify-send done");

    let saved = std::fs::read_to_string(&path).expect("saved config");
    assert!(saved.contains("notify-send done"));
    assert!(
        !saved.contains("medulla hook"),
        "a built-in reached the operator's config: {saved}"
    );
    assert_eq!(app.hook_rows().len(), builtins + 1);
}

#[test]
fn a_builtin_can_be_neither_edited_nor_deleted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let mut app = app_with_builtins(&path);
    app.hook_index = 0;
    assert!(app.hook_rows()[0].builtin, "row 0 should be a built-in");

    app.open_edit_hook();
    assert!(app.status().contains("cannot be edited"));

    let before = app.hook_rows().len();
    app.delete_selected_hook();
    assert_eq!(app.hook_rows().len(), before);
    assert!(app.status().contains("cannot be deleted"));
}

#[test]
fn turning_medullas_own_hooks_off_withdraws_them_here_and_on_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let mut app = app_with_builtins(&path);
    app.save_hook(None, "Stop |  |  |  | notify-send done");
    let with_builtins = app.hook_rows().len();

    app.toggle_builtin_hooks();
    assert!(!app.loaded.config.hook_defaults.enabled);
    assert_eq!(app.hook_rows().len(), 1, "only the operator's hook is left");
    assert!(std::fs::read_to_string(&path)
        .expect("saved config")
        .contains("enabled = false"));

    app.toggle_builtin_hooks();
    assert!(app.loaded.config.hook_defaults.enabled);
    assert_eq!(app.hook_rows().len(), with_builtins);
}
