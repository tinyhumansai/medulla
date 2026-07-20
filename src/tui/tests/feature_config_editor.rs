//! Feature tests for Settings > GENERAL > Config: the editable settings list
//! driven through real key events, and the effective-config view beneath it.
//!
//! The unit tests in `ui::app::settings_edit::tests` cover the value rules;
//! these cover the wiring — that keys reach the editor, that changes land in the
//! injected `config.toml`, and that both panels render.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::{LoadedConfig, TuiConfig};
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::App;

/// An app parked on the Config subpage, persisting into `dir/config.toml`.
fn config_app(dir: &std::path::Path) -> App {
    let rt = Arc::new(MockRuntime::demo());
    let mut app = App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()));
    app.set_config_path(dir.join("config.toml"));
    let _ = app.focus_settings_subpage("Config");
    app
}

fn key(app: &mut App, code: KeyCode) {
    app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

/// The config as written to disk.
fn written(dir: &std::path::Path) -> TuiConfig {
    let text = std::fs::read_to_string(dir.join("config.toml")).expect("config written");
    toml::from_str(&text).expect("valid toml")
}

#[test]
fn the_editor_lists_settings_over_the_effective_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = config_app(dir.path());
    let out = render(&mut app, 200, 60);

    for label in [
        "Persona memory",
        "Update check",
        "Max passes",
        "Worker concurrency",
    ] {
        assert!(out.contains(label), "editor missing {label}: {out}");
    }
    assert!(
        out.contains("Effective configuration ·"),
        "the read-only view is still there: {out}"
    );
}

#[test]
fn enter_toggles_the_selected_switch_and_writes_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = config_app(dir.path());

    // Persona memory is the first row and is off by default.
    key(&mut app, KeyCode::Enter);

    assert_eq!(
        written(dir.path()).memory.and_then(|m| m.enabled),
        Some(true),
        "the toggle reached disk"
    );
    let out = render(&mut app, 200, 60);
    assert!(out.contains("on"), "the new value renders: {out}");
}

#[test]
fn jk_moves_between_settings_without_leaving_the_subpage() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = config_app(dir.path());

    key(&mut app, KeyCode::Char('j'));
    assert_eq!(app.settings_subpage(), "Config", "j stays in the pane");

    // The second row is Update check, on by default; Enter turns it off.
    key(&mut app, KeyCode::Enter);
    assert!(!written(dir.path()).update.check, "the switch turned off");
}

#[test]
fn arrows_step_a_number_and_persist_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = config_app(dir.path());

    // Rows are: Persona memory, Update check, then the medulla limits — so two
    // presses of j land on Max passes. (No tiny.place row: it is unconfigured.)
    key(&mut app, KeyCode::Char('j'));
    key(&mut app, KeyCode::Char('j'));

    key(&mut app, KeyCode::Right);
    let first = written(dir.path()).medulla.max_passes.expect("persisted");

    key(&mut app, KeyCode::Right);
    let second = written(dir.path()).medulla.max_passes.expect("persisted");
    assert_eq!(second, first + 1, "each → steps once");

    key(&mut app, KeyCode::Left);
    assert_eq!(
        written(dir.path()).medulla.max_passes,
        Some(first),
        "← steps back"
    );
}

#[test]
fn arrows_never_leave_the_subpage() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = config_app(dir.path());
    for _ in 0..5 {
        key(&mut app, KeyCode::Left);
        key(&mut app, KeyCode::Right);
    }
    assert_eq!(
        app.settings_subpage(),
        "Config",
        "←→ edit the value rather than moving the nav"
    );
}

#[test]
fn up_and_down_follow_focus_between_the_nav_and_the_page() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = config_app(dir.path());

    // Focused on the page, arrows pick a setting and leave the nav alone.
    assert!(app.settings_focused(), "config_app enters the page");
    key(&mut app, KeyCode::Down);
    assert_eq!(
        app.settings_subpage(),
        "Config",
        "arrows stay inside the focused page"
    );

    // Back on the nav, the same keys walk the subpage list again.
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.settings_subpage(), "Feedback");
    key(&mut app, KeyCode::Up);
    assert_eq!(app.settings_subpage(), "Config");
}
