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

/// An app whose live operator-launch state (`local_sessions`) is populated, the
/// way a hosting device's really is — see `app_loop::run_tui`'s
/// `LocalSessions` construction.
fn app_hosting(path: &std::path::Path) -> App {
    let mut app = app_with_config(path);
    app.local_sessions = Some(crate::ui::harness_pane::LocalSessions {
        sessions: crate::worker::pty::PtyManager::new(),
        runtimes: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        hub_address: "this-device".to_string(),
        env: std::collections::HashMap::new(),
        workspace: "/repos/acme".to_string(),
        providers: Vec::new(),
        custom_harnesses: Vec::new(),
        router: None,
        attribution: true,
        hooks: app.loaded.config.hooks.clone(),
        log: None,
    });
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

/// A config-declared operator hook can carry a `label` the editor form has
/// no field for. Editing that hook must not silently drop it — the parsed
/// line always yields `label: None`, so `save_hook` has to carry the row's
/// existing label forward itself.
#[test]
fn editing_a_labeled_hook_preserves_its_label() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let mut app = app_with_config(&path);
    app.loaded.config.hooks.hooks.push(medulla::harness_hooks::HookSpec::from_editor_line(
        "Stop |  |  |  | notify-send done",
    )
    .expect("valid editor line"));
    app.loaded.config.hooks.hooks[0].label = Some("My hook".to_string());

    app.save_hook(Some(0), "Stop |  |  | 5 | notify-send done");

    let hook = &app.hook_rows()[0];
    assert_eq!(hook.label.as_deref(), Some("My hook"));
    assert_eq!(hook.timeout(), Some(5));
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

/// The P2 Codex found on this branch: saving a hook updated
/// `loaded.config.hooks`, but `LocalSessions.hooks` — the copy
/// `harness_pane::spawn::open_unmanaged` actually reads when the operator
/// starts a harness from the Agents pane — was a separate clone taken once at
/// startup and never touched again. The status line claimed "applies to
/// harnesses started from now on" while a session opened before a restart
/// still launched with the stale hook set: a newly added hook would not run,
/// and a removed one would keep running.
#[test]
fn saving_a_hook_updates_the_live_operator_launch_state_too() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let mut app = app_hosting(&path);
    assert!(
        app.local_sessions.as_ref().unwrap().hooks.hooks.is_empty(),
        "starts with no hooks"
    );

    app.save_hook(None, "Stop |  |  |  | notify-send done");

    let live = &app.local_sessions.as_ref().unwrap().hooks;
    assert_eq!(
        live.hooks, app.loaded.config.hooks.hooks,
        "the live launch state must match what was just saved"
    );
    assert!(
        live.hooks.iter().any(|h| h.command() == "notify-send done"),
        "the freshly added hook must reach the live launch state: {live:?}"
    );
}

/// The same staleness class applies to turning Medulla's own hooks off: the
/// operator flips `b`, the status line says the harnesses report nothing —
/// that has to be true for a session opened from the Agents pane right after,
/// not only for one started following a restart.
#[test]
fn toggling_builtin_hooks_updates_the_live_operator_launch_state_too() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let mut app = app_hosting(&path);
    app.loaded.config.hooks = HooksConfig::default()
        .with_builtin(medulla::harness_hooks::builtin::hooks("/usr/bin/medulla"));
    app.local_sessions.as_mut().unwrap().hooks = app.loaded.config.hooks.clone();
    let with_builtins = app.local_sessions.as_ref().unwrap().hooks.hooks.len();
    assert!(with_builtins > 0);

    app.toggle_builtin_hooks();

    let live = &app.local_sessions.as_ref().unwrap().hooks;
    assert_eq!(
        live.hooks, app.loaded.config.hooks.hooks,
        "the live launch state must match what was just toggled"
    );
    assert!(
        live.hooks.is_empty(),
        "no operator hooks were declared, so turning built-ins off must leave \
         the live launch state empty too: {live:?}"
    );
}

/// The P2 Codex found on this branch: `config_path` can resolve to a
/// project-local file (`.medulla/config.toml` / `medulla.toml`), and
/// `medulla::config::load_config` strips `[[hooks]]` from exactly that layer
/// on every non-explicit load. A hook saved there would show "Hook saved" and
/// vanish on the next launch, so `persist_hooks` must target a separate,
/// always-honored path instead — see `App::hooks_config_path`.
#[test]
fn a_hook_saved_while_a_project_local_config_is_active_still_lands_where_the_next_load_reads_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project_path = dir.path().join("medulla.toml");
    std::fs::write(&project_path, "[theme]\naccent = \"blue\"\n").expect("seed project config");
    let global_path = dir.path().join("global-config.toml");
    std::fs::write(&global_path, "[theme]\naccent = \"red\"\n").expect("seed global config");

    let loaded = medulla::config::LoadedConfig::defaults(project_path.display().to_string());
    let mut app = App::new(Arc::new(MockRuntime::demo()), loaded);
    // `set_config_path` is what other settings (appearance, routing, …) save
    // through, and it is the project-local file here — exactly the case
    // `app_loop::run_tui` hits when `loaded.sources.last()` is a project-local
    // layer. `set_hooks_config_path` is the production seam that redirects
    // hooks alone to the file `load_config` will not strip them from.
    app.set_config_path(project_path.clone());
    app.set_hooks_config_path(global_path.clone());

    app.save_hook(None, "Stop |  |  |  | notify-send done");

    let project_saved = std::fs::read_to_string(&project_path).expect("project config");
    assert!(
        !project_saved.contains("notify-send done"),
        "a hook must never be written to the layer load_config strips it from: {project_saved}"
    );
    let global_saved = std::fs::read_to_string(&global_path).expect("global config");
    assert!(
        global_saved.contains("notify-send done"),
        "the hook must land in the file the next load actually reads: {global_saved}"
    );
    assert!(
        app.status()
            .contains("project config cannot authorize hooks"),
        "the operator must be told the save landed elsewhere: {}",
        app.status()
    );
}
